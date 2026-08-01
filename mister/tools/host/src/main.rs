// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_tool::transport::{
    DeviceFailure, DeviceOperations, DeviceRequest, DeviceResponse, Layout,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ssh2::Session;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod agent_client;
mod arcade_database;
mod crt_qualification;
mod discovery;
mod latch_v4_qualification;
mod launcher_automation;
mod media;
mod platform_deploy;
mod remote;

use agent_client::{
    AGENT_PORT, AgentEndpoint, agent_binary_request_bounded, agent_request, agent_request_at,
    agent_request_with_liveness, agent_telemetry_for_duration,
    agent_telemetry_for_particle_renderer_trial, agent_telemetry_for_particle_trial,
    agent_telemetry_until_screensaver_profile_complete, agent_token, agent_token_for_device,
    bootstrap_agent, bootstrap_agent_with,
};
use platform_deploy::*;
use remote::{
    ConnectionConfig, ExecOutput, acknowledged_main_command, connect, connect_timed,
    connect_timed_with, connect_with, create_dir_command, exec, exec_failure_message, get, host,
    host_wait_diagnostics_with, launcher_restart_command, port_open, port_open_with, put,
    put_bytes, remote_subcommand, remove_files_command, sftp_write_profile, shell_quote as sh,
    tcp_probe_label, tcp_probe_label_port,
};

#[cfg(test)]
const DEFAULT_FB_W: usize = 1920;
#[cfg(test)]
const DEFAULT_FB_H: usize = 1080;
#[cfg(test)]
const DEFAULT_FB_BPP: usize = 32;
const MAX_FRAMEBUFFER_CAPTURE_RAW_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAMEBUFFER_CAPTURE_PAYLOAD_BYTES: u64 = 17 * 1024 * 1024;
const RAW_REBOOT_REMOTE_CMD: &str = "nohup /sbin/reboot >/dev/null 2>&1 & echo raw";
const DIRECT_RESET_REMOTE_CMD: &str = "if [ -p /dev/MiSTer_cmd ] && { pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }; then exec 8>/tmp/mister-magik/command-operation.lock; flock 8; printf 'mister_magik_direct_reset\\n' > /dev/MiSTer_cmd; echo direct-reset; else echo 'direct reset unavailable: MagiK Main or /dev/MiSTer_cmd missing' >&2; exit 12; fi";
const DIRECT_RESET_NO_SYNC_REMOTE_CMD: &str = "if [ -p /dev/MiSTer_cmd ] && { pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }; then exec 8>/tmp/mister-magik/command-operation.lock; flock 8; printf 'mister_magik_direct_reset_no_sync\\n' > /dev/MiSTer_cmd; echo direct-reset-no-sync; else echo 'direct reset unavailable: MagiK Main or /dev/MiSTer_cmd missing' >&2; exit 12; fi";
#[cfg(test)]
const DEFAULT_REMOTE_LIBRARY_DB: &str = "/media/fat/mister-magik/library.sqlite3";
const DEFAULT_LAUNCHER_ENV_REMOTE: &str = "/media/fat/mister-magik/launcher.env";
const DEVELOPMENT_LAUNCHER_ENV_REMOTE: &str = "/media/fat/mister-magik-dev/launcher.env";
const RETURN_CATALOG_CAPSULE_REMOTE: &str = "/tmp/mister-magik/launcher-return-catalog.json";
const MAIN_STATUS_REMOTE: &str = "/tmp/mister-magik/main-status.json";
const SLINT_STATUS_REMOTE: &str = "/tmp/mister-magik/status.json";
const LATCH_FAILURE_REMOTE: &str = "/tmp/mister-magik/latch-failure.json";
const RESOLVED_DEVICE_CHILD: &str = "MISTER_MAGIK_RESOLVED_DEVICE_CHILD";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn configured_remote_path(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RebootMode {
    Supervised,
    Raw,
    DirectReset,
    DirectResetNoSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SdListProtocol {
    Auto,
    V1,
    V2,
}

impl SdListProtocol {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "v1" => Ok(Self::V1),
            "v2" => Ok(Self::V2),
            _ => Err(format!("unsupported SD list protocol: {value}").into()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::V1 => "v1",
            Self::V2 => "v2",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct SdListOptions {
    path: String,
    protocol: SdListProtocol,
    show_hidden: bool,
    repeat: usize,
    json: bool,
}

impl RebootMode {
    fn label(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Raw => "raw",
            Self::DirectReset => "direct-reset",
            Self::DirectResetNoSync => "direct-reset-no-sync",
        }
    }

    fn is_direct_reset(self) -> bool {
        matches!(self, Self::DirectReset | Self::DirectResetNoSync)
    }
}

#[allow(dead_code)]
fn main() {
    if let Err(e) = run_cli() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[derive(Clone, Debug)]
struct NativeDeviceConfig {
    connection: ConnectionConfig,
    device_id: String,
    agent: AgentEndpoint,
}

impl NativeDeviceConfig {
    fn new(connection: ConnectionConfig, device_id: String, token: String) -> Self {
        let agent = AgentEndpoint::new(connection.host(), token);
        Self {
            connection,
            device_id,
            agent,
        }
    }
}

#[derive(Default)]
pub struct NativeDevice {
    config: Option<NativeDeviceConfig>,
}

struct DeliveryProcessLock {
    file: fs::File,
}

impl DeliveryProcessLock {
    fn acquire(device_id: &str) -> std::result::Result<Self, DeviceFailure> {
        let safe_id = device_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("mister-magik-delivery-{safe_id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                DeviceFailure::OperationFailed(format!(
                    "cannot open delivery lock {}: {error}",
                    path.display()
                ))
            })?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(DeviceFailure::Busy(
                "another delivery process is already running".into(),
            ));
        }
        Ok(Self { file })
    }
}

impl Drop for DeliveryProcessLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

impl NativeDevice {
    fn prepare(&mut self) -> std::result::Result<(), DeviceFailure> {
        if self.config.is_some() {
            return Ok(());
        }
        let device = discovery::resolve().map_err(device_failure)?;
        let connection = ConnectionConfig::for_resolved_host(device.address.to_string());
        let explicit_token = env::var("MISTER_AGENT_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        let token = if std::env::var_os("MISTER_SKIP_AGENT_BOOTSTRAP").is_none() {
            bootstrap_agent_with(&connection, &device.id, explicit_token.as_deref())
                .map_err(device_failure)?
        } else {
            agent_token_for_device(&device.id, explicit_token.as_deref()).map_err(device_failure)?
        };
        self.config = Some(NativeDeviceConfig::new(connection, device.id, token));
        Ok(())
    }
}

impl DeviceOperations for NativeDevice {
    fn execute(
        &mut self,
        request: &DeviceRequest,
    ) -> std::result::Result<DeviceResponse, DeviceFailure> {
        self.prepare()?;
        let config = self.config.clone().ok_or_else(|| {
            DeviceFailure::OperationFailed("device configuration is unavailable".into())
        })?;
        debug_assert!(!config.device_id.is_empty());
        let connect = |timeout_secs| connect_with(&config.connection, timeout_secs);
        let detail = match request {
            DeviceRequest::Discover => "connected".into(),
            DeviceRequest::Status => {
                let session = connect(10).map_err(device_failure)?;
                serde_json::to_string(&collect_status(&session).map_err(device_failure)?)
                    .map_err(device_failure)?
            }
            DeviceRequest::ReadDevelopmentManifest => {
                let session = connect(10).map_err(device_failure)?;
                remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
                    .unwrap_or_default()
            }
            DeviceRequest::VerifyDevelopmentPlatform => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "development platform verify",
                    &installed_platform_verify_command(Layout::Development),
                )
                .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
                "verified".into()
            }
            DeviceRequest::FetchVerifiedDevelopmentManager {
                local,
                expected_sha256,
            } => fetch_verified_development_manager(&config, local, expected_sha256)?,
            DeviceRequest::DeliverRuntimeTransaction {
                local,
                remote,
                manifest_local,
                manifest_remote,
                expected_sha256,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                deliver_runtime_transaction(
                    &config,
                    local,
                    remote,
                    manifest_local,
                    manifest_remote,
                    expected_sha256,
                )?
            }
            DeviceRequest::DeliverPlatformTransaction {
                stage,
                expected_sha256,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                deliver_platform_transaction(&config, stage, expected_sha256)?
            }
            DeviceRequest::DeliverLocalMainTransaction {
                local,
                manifest_local,
                expected_main_sha256,
                expected_gui_sha256,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                deliver_local_main_transaction(
                    &config,
                    local,
                    manifest_local,
                    expected_main_sha256,
                    expected_gui_sha256,
                )?
            }
            DeviceRequest::ProfileInstalledScreensaver { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_screensaver(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticles { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_particles(&config, output_dir, ParticleBenchmarkRun::Complete)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticleCapacity { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_particles(&config, output_dir, ParticleBenchmarkRun::Capacity)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticleDemo40k { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_particles(&config, output_dir, ParticleBenchmarkRun::Demo40k)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticleStep { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_particles(&config, output_dir, ParticleBenchmarkRun::Step)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticleCpu { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_particle_cpu(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledParticleShowcase {
                output_dir,
                demo,
                cpu_profile,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                if *cpu_profile {
                    profile_installed_particle_showcase_cpu(&config, output_dir, *demo)
                        .map_err(device_failure)?
                } else {
                    profile_installed_particles(
                        &config,
                        output_dir,
                        ParticleBenchmarkRun::Showcase(*demo),
                    )
                    .map_err(device_failure)?
                }
            }
            DeviceRequest::CaptureInstalledFireworkVisual {
                output_dir,
                demo,
                label,
                time_ms,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                capture_installed_firework_visual(&config, output_dir, *demo, label, *time_ms)
                    .map_err(device_failure)?
            }
            DeviceRequest::CaptureInstalledParticleTechnique {
                output_dir,
                demo,
                label,
                hero_secs,
            } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                capture_installed_particle_technique(&config, output_dir, *demo, label, *hero_secs)
                    .map_err(device_failure)?
            }
            DeviceRequest::LaunchParticleShowcase => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                launch_particle_showcase_interactive(&config).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledSearch { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_search(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::VerifyInstalledSearchUi { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                verify_installed_search_ui(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledCatalogLifecycle { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_catalog_lifecycle(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledLaunchReturn { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_launch_return(&config, output_dir, false)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledLaunchReturnFallback { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_launch_return(&config, output_dir, true)
                    .map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledColdBoot { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_cold_boot(&config, output_dir).map_err(device_failure)?
            }
            DeviceRequest::ProfileInstalledNavigationTransitions { output_dir } => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                profile_installed_navigation_transitions(&config, output_dir)
                    .map_err(device_failure)?
            }
            DeviceRequest::VerifyHealth(layout) => {
                let label = match layout {
                    Layout::Development => "dev",
                    Layout::Public => "public",
                };
                let session = connect(10).map_err(device_failure)?;
                wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                wait_delivery_health(&session, label, Duration::from_secs(10))
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                "healthy".into()
            }
            DeviceRequest::BeginReleaseQualification => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "release recovery preflight",
                    &release_begin_command(),
                )
                .map_err(device_failure)?;
                "volatile-token=armed recovery=confirmed".into()
            }
            DeviceRequest::QualifyReleaseRuntime => {
                let session = connect(10).map_err(device_failure)?;
                let command = format!(
                    "if pidof MiSTer_MagiKDev >/dev/null 2>&1; then {}; else {}; fi",
                    delivery_health_command("dev").map_err(device_failure)?,
                    delivery_health_command("public").map_err(device_failure)?
                );
                exec_checked(&session, "release runtime", &command).map_err(device_failure)?;
                "runtime=healthy".into()
            }
            DeviceRequest::QualifyReleaseCatalog => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "release catalog", &release_catalog_command())
                    .map_err(device_failure)?;
                "catalog=valid".into()
            }
            DeviceRequest::QualifyReleaseInputAndHandoff => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "release input and handoff",
                    &release_handoff_command(),
                )
                .map_err(device_failure)?;
                "input=ready handoff=ready return=ready".into()
            }
            DeviceRequest::QualifyReleaseDisplay => {
                qualify_release_display_matrix_with(&config.connection, &config.agent)
                    .map_err(device_failure)?
            }
            DeviceRequest::QualifyReleaseLatchV4Stress => {
                latch_v4_qualification::run(&config).map_err(device_failure)?
            }
            DeviceRequest::QualifyReleaseRecovery => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "release recovery", &release_recovery_command())
                    .map_err(device_failure)?;
                "recovery=qualified token=volatile".into()
            }
            DeviceRequest::RestoreReleaseQualification => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "release restore", &release_restore_command())
                    .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
                issue_reboot(&session, RebootMode::Supervised)
                    .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
                drop(session);
                if !wait_down_with(&config.connection, 40.0)
                    || wait_up_with(&config.connection, 120.0)
                        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?
                        != 0
                {
                    return Err(DeviceFailure::RecoveryRequired(
                        "device did not reboot after restoring release configuration".into(),
                    ));
                }
                "restored arming=clear".into()
            }
            DeviceRequest::CollectDiagnosticFacts => {
                let session = connect(10).map_err(device_failure)?;
                let output =
                    exec(&session, &diagnostic_facts_command(), false).map_err(device_failure)?;
                if let Some(message) = exec_failure_message("diagnostic facts", &output) {
                    return Err(device_failure(message));
                }
                let mut facts: Value =
                    serde_json::from_str(output.stdout.trim()).map_err(device_failure)?;
                if let Some(main_status) = remote_read(&session, MAIN_STATUS_REMOTE)
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    && let (Some(facts), Some(main)) =
                        (facts.as_object_mut(), main_status.as_object())
                {
                    for key in [
                        "launcher_state",
                        "crash_count",
                        "last_crash_reason",
                        "last_crash_report",
                        "last_crash_report_id",
                        "last_crash_kind",
                    ] {
                        if let Some(value) = main.get(key) {
                            facts.insert(key.to_owned(), value.clone());
                        }
                    }
                }
                if let Some(slint_status) = remote_read(&session, SLINT_STATUS_REMOTE)
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    && let (Some(facts), Some(slint)) =
                        (facts.as_object_mut(), slint_status.as_object())
                {
                    for key in [
                        "scene",
                        "screen",
                        "effective_view",
                        "return_screen",
                        "input_enabled",
                        "present_backend",
                        "present_status",
                        "latch_failure_state",
                        "latch_failure_stage",
                        "latch_failure_reason",
                        "latch_failure_detail",
                        "compatibility_prompt_visible",
                    ] {
                        if let Some(value) = slint.get(key) {
                            facts.insert(key.to_owned(), value.clone());
                        }
                    }
                }
                if let Some(latch_failure) = remote_read(&session, LATCH_FAILURE_REMOTE)
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    && let (Some(facts), Some(failure)) =
                        (facts.as_object_mut(), latch_failure.as_object())
                {
                    for (fact_key, failure_key) in [
                        ("latch_failure_state", "state"),
                        ("latch_failure_stage", "stage"),
                        ("latch_failure_reason", "reason"),
                        ("latch_failure_detail", "detail"),
                        ("latch_latest_state", "latest_state"),
                        ("latch_latest_stage", "latest_stage"),
                        ("latch_latest_reason", "latest_reason"),
                        ("latch_latest_detail", "latest_detail"),
                        ("latch_recovery_attempt_count", "attempt_count"),
                        ("latch_latest_retry_result", "latest_result"),
                        ("latch_recovery_state", "recovery_state"),
                    ] {
                        if let Some(value) = failure.get(failure_key) {
                            facts.insert(fact_key.to_owned(), value.clone());
                        }
                    }
                }
                if let Some(facts) = facts.as_object_mut() {
                    match request_framebuffer_png_at(&config.agent) {
                        Ok(capture) => {
                            facts.insert(
                                "capture_source".into(),
                                Value::String(
                                    capture_source_label(&capture.result)
                                        .map_err(device_failure)?
                                        .to_owned(),
                                ),
                            );
                            facts.insert(
                                "capture_authoritative_scanout".into(),
                                Value::Bool(
                                    capture
                                        .result
                                        .get("authoritative_scanout")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false),
                                ),
                            );
                        }
                        Err(error) => {
                            facts.insert("capture_error".into(), Value::String(error.to_string()));
                        }
                    }
                }
                let evidence_dir =
                    retain_diagnostic_evidence(&session, &facts).map_err(device_failure)?;
                if let Some(facts) = facts.as_object_mut() {
                    facts.insert(
                        "evidence_dir".into(),
                        Value::String(evidence_dir.display().to_string()),
                    );
                }
                serde_json::to_string(&facts).map_err(device_failure)?
            }
            DeviceRequest::ClearLatchDiagnostics => {
                let _lock = DeliveryProcessLock::acquire(&config.device_id)?;
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "latch diagnostics cleanup",
                    clear_latch_diagnostics_command(),
                )
                .map_err(device_failure)?;
                "latch diagnostics cleared and verified".into()
            }
            DeviceRequest::CollectLatestCrashReport => {
                let session = connect(10).map_err(device_failure)?;
                let main_status = remote_read(&session, MAIN_STATUS_REMOTE)
                    .ok_or_else(|| DeviceFailure::Unhealthy("Main status is unavailable".into()))?;
                let main_status: Value =
                    serde_json::from_str(&main_status).map_err(device_failure)?;
                let path = main_status
                    .get("last_crash_report")
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| {
                        DeviceFailure::Unhealthy("Main has no recorded crash report".into())
                    })?;
                if !is_safe_crash_report_path(path) {
                    return Err(DeviceFailure::InvalidRequest(format!(
                        "Main reported an invalid crash-report path: {path}"
                    )));
                }
                let report = remote_read(&session, path).ok_or_else(|| {
                    DeviceFailure::Unhealthy(format!("Crash report is missing: {path}"))
                })?;
                let report: Value = serde_json::from_str(&report).map_err(device_failure)?;
                if report.get("schema").and_then(Value::as_str)
                    != Some("mister-magik-crash-report-v1")
                {
                    return Err(DeviceFailure::Unhealthy(
                        "Crash report has an unsupported schema".into(),
                    ));
                }
                serde_json::to_string(&json!({"path": path, "report": report}))
                    .map_err(device_failure)?
            }
            DeviceRequest::RunCrtGeometryTrial { rectangle } => {
                run_crt_geometry_trial_with(&config.connection, *rectangle)
                    .map_err(device_failure)?
            }
            DeviceRequest::RunCrtScreensaverTrial => {
                run_crt_screensaver_trial_with(&config.connection, 30, 10)
                    .map_err(device_failure)?
            }
            DeviceRequest::RunCrtScreensaverMatrix => {
                run_crt_screensaver_matrix_with(&config.connection).map_err(device_failure)?
            }
            DeviceRequest::RepairSafeDeviceState => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "safe diagnostic repair", &safe_repair_command())
                    .map_err(device_failure)?;
                "temporary-state=clear launcher=unchanged".into()
            }
            DeviceRequest::RecoverWithOneShotReboot => {
                one_shot_recovery_reboot_wait(&config)?;
                "reboot=raw recovery=healthy arming=clear".into()
            }
            DeviceRequest::CaptureFramebuffer => {
                capture_buffer_at(&config.agent, &[]).map_err(device_failure)?;
                "captured".into()
            }
            DeviceRequest::InstallAlphaCandidate {
                tag,
                hashes,
                restore_on_failure,
            } => install_alpha_candidate(&config, tag, hashes, *restore_on_failure)?,
            DeviceRequest::RestoreAlphaHostMode { original_main } => {
                restore_alpha_host_mode(&config, original_main.clone())?
            }
            DeviceRequest::EnsureInstalledAlphaLauncher {
                expected_build_version,
                expected_source_revision,
            } => launcher_automation::ensure_installed_alpha_launcher(
                &config,
                expected_build_version,
                expected_source_revision,
            )
            .map_err(device_failure)?,
            DeviceRequest::InspectPublicCatalog => {
                let session = connect(10).map_err(device_failure)?;
                let inspect = exec(
                    &session,
                    "/media/fat/mister-magik/mister-magik-fb catalog-v3-inspect",
                    true,
                )
                .map_err(device_failure)?;
                if let Some(error) = exec_failure_message("public catalog inspect", &inspect) {
                    return Err(DeviceFailure::Unhealthy(error));
                }
                serde_json::to_string(
                    &parse_catalog_lifecycle_inspect(&inspect.stdout).map_err(device_failure)?,
                )
                .map_err(device_failure)?
            }
            DeviceRequest::BeginLauncherAutomation {
                expected_build_version,
                expected_source_revision,
                expected_main_generation,
                lifetime_seconds,
            } => launcher_automation::begin(
                &config,
                expected_build_version,
                expected_source_revision,
                *expected_main_generation,
                *lifetime_seconds,
            )
            .map_err(device_failure)?,
            DeviceRequest::SendLauncherAutomationAction { nonce, action } => {
                launcher_automation::send_action(&config, nonce, action).map_err(device_failure)?
            }
            DeviceRequest::AwaitLauncherAutomationPresented {
                nonce,
                action_sequence,
                timeout_ms,
            } => {
                launcher_automation::await_presented(&config, nonce, *action_sequence, *timeout_ms)
                    .map_err(device_failure)?
            }
            DeviceRequest::ReadLauncherAutomationSnapshot { nonce } => serde_json::to_string(
                &launcher_automation::snapshot(&config, nonce).map_err(device_failure)?,
            )
            .map_err(device_failure)?,
            DeviceRequest::CaptureLauncherAutomationCheckpoint {
                nonce,
                action_sequence,
                label,
                output_dir,
            } => launcher_automation::capture_checkpoint(
                &config,
                nonce,
                *action_sequence,
                label,
                output_dir,
            )
            .map_err(device_failure)?,
            DeviceRequest::ExerciseLauncherAutomationLaunchReturn {
                nonce,
                expected_game_id,
                lifetime_seconds,
            } => match launcher_automation::exercise_launch_return(
                &config,
                nonce,
                expected_game_id,
                *lifetime_seconds,
            ) {
                Ok(detail) => detail,
                Err(launcher_automation::LaunchReturnError::Failed(detail)) => {
                    return Err(DeviceFailure::OperationFailed(detail));
                }
                Err(launcher_automation::LaunchReturnError::RecoveryRequired(detail)) => {
                    return Err(DeviceFailure::RecoveryRequired(detail));
                }
            },
            DeviceRequest::EndLauncherAutomation { nonce } => {
                launcher_automation::end(&config, nonce).map_err(device_failure)?
            }
        };
        Ok(DeviceResponse {
            operation: request.label(),
            detail,
        })
    }
}

fn install_alpha_candidate(
    config: &NativeDeviceConfig,
    tag: &str,
    hashes: &mister_tool::transport::AlphaCandidateHashes,
    restore_on_failure: bool,
) -> std::result::Result<String, DeviceFailure> {
    for hash in [
        &hashes.platform_manifest,
        &hashes.main,
        &hashes.gui,
        &hashes.manager,
        &hashes.scanout_module,
        &hashes.scanout_metadata,
        &hashes.latch_rbf,
        &hashes.latch_metadata,
    ] {
        require_delivery_sha256(hash)?;
    }
    let reply = agent_request_at(
        &config.agent,
        "alpha_candidate_install",
        json!({
            "tag": tag,
            "platform_manifest_sha256": hashes.platform_manifest,
            "component_sha256": {
                "main": hashes.main,
                "gui": hashes.gui,
                "manager": hashes.manager,
                "scanout_module": hashes.scanout_module,
                "scanout_metadata": hashes.scanout_metadata,
                "latch_rbf": hashes.latch_rbf,
                "latch_metadata": hashes.latch_metadata,
            },
        }),
        Duration::from_secs(250),
    )
    .map_err(device_failure)?;
    let installed =
        reply.response.get("result").cloned().ok_or_else(|| {
            DeviceFailure::OperationFailed("candidate install has no result".into())
        })?;
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    exec_checked(
        &session,
        "alpha candidate public platform verification before activation",
        &installed_platform_verify_command(Layout::Public),
    )
    .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
    let original_main = alpha_host_main(&session)?;
    require_alpha_host_main(original_main.as_deref())?;
    ensure_stock_inittab(&session, false).map_err(device_failure)?;
    if let Err(error) = edit_remote_ini(&session, IniEdit::SelectMain("MiSTer_MagiK".into()), false)
    {
        let primary = DeviceFailure::OperationFailed(error.to_string());
        if !restore_on_failure {
            return Err(primary);
        }
        return match restore_alpha_host_mode(config, original_main) {
            Ok(_) => Err(primary),
            Err(restore) => Err(alpha_restore_failure(primary, restore)),
        };
    }
    let activation = (|| -> std::result::Result<Value, DeviceFailure> {
        let safety = platform_safety_script();
        let cleanup =
            shell_sequence(["set -eu", release_arming_cleanup_command(), safety.as_str()]);
        exec_checked(&session, "alpha activation arming cleanup", &cleanup)
            .map_err(device_failure)?;
        issue_delivery_reboot(&session).map_err(device_failure)?;
        drop(session);
        if !wait_down_with(&config.connection, 40.0)
            || wait_up_with(&config.connection, 120.0).map_err(device_failure)? != 0
        {
            return Err(DeviceFailure::Unavailable(
                "device did not complete its alpha activation reboot".into(),
            ));
        }
        let session = connect_with(&config.connection, 10).map_err(device_failure)?;
        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
            .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
        exec_checked(
            &session,
            "alpha candidate public platform verification",
            &installed_platform_verify_command(Layout::Public),
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        wait_delivery_health(&session, "public", Duration::from_secs(10))
            .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
        wait_alpha_catalog_ready(
            &session,
            Duration::from_secs(ALPHA_CATALOG_COMPLETE_TIMEOUT_SECS),
        )
        .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))
    })();
    let catalog = match activation {
        Ok(catalog) => catalog,
        Err(primary) => {
            if !restore_on_failure {
                return Err(primary);
            }
            return match restore_alpha_host_mode(config, original_main) {
                Ok(_) => Err(primary),
                Err(restore) => Err(alpha_restore_failure(primary, restore)),
            };
        }
    };
    serde_json::to_string(&json!({
        "schema": "mister-magik-alpha-candidate-activation-v1",
        "install": installed,
        "supervised_reboot": true,
        "public_health": "ready",
        "catalog": catalog,
        "original_main": original_main,
    }))
    .map_err(device_failure)
}

fn wait_alpha_catalog_ready(session: &Session, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    let started_at_unix_ms = unix_ms_now();
    let mut last = Value::Null;
    let mut initial_ready = None;
    let mut initial_refresh_done = None;
    let mut first_visible_ms = None;
    let mut last_progress_second = u64::MAX;
    loop {
        if let Some(status) = remote_read(session, "/tmp/mister-magik/status.json")
            .and_then(|status| serde_json::from_str::<Value>(&status).ok())
        {
            let ready = status.get("catalog_ready").and_then(Value::as_bool);
            let refresh_done = status.get("catalog_refresh_done").and_then(Value::as_bool);
            initial_ready.get_or_insert(ready.unwrap_or(false));
            initial_refresh_done.get_or_insert(refresh_done.unwrap_or(false));
            if first_visible_ms.is_none() && ready == Some(true) {
                first_visible_ms = Some(started.elapsed().as_millis() as u64);
            }
            if ready == Some(true)
                && status
                    .get("catalog_games")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            {
                return Ok(json!({
                    "schema": "mister-magik-alpha-catalog-start-v1",
                    "mode": if initial_ready == Some(false) || initial_refresh_done == Some(false) {
                        "built-or-upgraded"
                    } else {
                        "cached"
                    },
                    "started_at_unix_ms": started_at_unix_ms,
                    "deadline_unix_ms": started_at_unix_ms.saturating_add(timeout.as_millis()),
                    "initial_catalog_ready": initial_ready,
                    "initial_refresh_done": initial_refresh_done,
                    "timing": {
                        "first_visible_ms": first_visible_ms,
                    },
                    "first_visible": {
                        "generation": status.get("catalog_generation"),
                        "games": status.get("catalog_games"),
                        "systems": status.get("catalog_systems"),
                        "refresh_done": refresh_done,
                    },
                }));
            }
            last = json!({
                "catalog_ready": status.get("catalog_ready"),
                "catalog_generation": status.get("catalog_generation"),
                "catalog_games": status.get("catalog_games"),
                "catalog_systems": status.get("catalog_systems"),
                "catalog_scan_visible": status.get("catalog_scan_visible"),
                "catalog_scan_percent": status.get("catalog_scan_percent"),
                "catalog_refresh_done": status.get("catalog_refresh_done"),
            });
            let elapsed_second = started.elapsed().as_secs();
            if elapsed_second / 5 != last_progress_second / 5 {
                eprintln!(
                    "alpha catalog build elapsed={}s ready={} refresh_done={} games={} systems={} percent={}",
                    elapsed_second,
                    ready.map_or("?".into(), |value| value.to_string()),
                    refresh_done.map_or("?".into(), |value| value.to_string()),
                    status
                        .get("catalog_games")
                        .map_or("?".into(), Value::to_string),
                    status
                        .get("catalog_systems")
                        .map_or("?".into(), Value::to_string),
                    status
                        .get("catalog_scan_percent")
                        .map_or("?".into(), Value::to_string),
                );
                last_progress_second = elapsed_second;
            }
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "public alpha catalog did not become ready within {}s; last_status={last}",
                timeout.as_secs()
            )
            .into());
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn alpha_host_main(session: &Session) -> std::result::Result<Option<String>, DeviceFailure> {
    let input = remote_read(session, "/media/fat/MiSTer.ini")
        .ok_or_else(|| DeviceFailure::OperationFailed("MiSTer.ini is unavailable".into()))?;
    let document = mister_magik_ini::Document::parse(input.as_bytes()).map_err(device_failure)?;
    Ok(document.effective_value("MiSTer", "main"))
}

fn require_alpha_host_main(value: Option<&str>) -> std::result::Result<(), DeviceFailure> {
    if value.is_none_or(|value| matches!(value, "MiSTer" | "MiSTer_MagiK" | "MiSTer_MagiKDev")) {
        Ok(())
    } else {
        Err(DeviceFailure::InvalidRequest(format!(
            "alpha acceptance cannot safely restore unsupported Main selection: {}",
            value.unwrap_or_default()
        )))
    }
}

fn restore_alpha_host_mode(
    config: &NativeDeviceConfig,
    original_main: Option<String>,
) -> std::result::Result<String, DeviceFailure> {
    require_alpha_host_main(original_main.as_deref())?;
    let session = connect_with(&config.connection, 10)
        .or_else(|_| {
            wait_up_with(&config.connection, 120.0)?;
            connect_with(&config.connection, 10)
        })
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    if alpha_host_main(&session)
        .map_err(|error| DeviceFailure::RecoveryRequired(format!("{error:?}")))?
        == original_main
    {
        return Ok("host-mode=unchanged".into());
    }
    ensure_stock_inittab(&session, false)
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    edit_remote_ini(&session, IniEdit::RestoreMain(original_main.clone()), false)
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    let safety = platform_safety_script();
    let cleanup = shell_sequence(["set -eu", release_arming_cleanup_command(), safety.as_str()]);
    exec_checked(&session, "alpha restore arming cleanup", &cleanup)
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    issue_delivery_reboot(&session)
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    drop(session);
    if !wait_down_with(&config.connection, 40.0)
        || wait_up_with(&config.connection, 120.0)
            .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?
            != 0
    {
        return Err(DeviceFailure::RecoveryRequired(
            "device did not complete its alpha host-mode restore reboot".into(),
        ));
    }
    let session = connect_with(&config.connection, 10)
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
    if alpha_host_main(&session)
        .map_err(|error| DeviceFailure::RecoveryRequired(format!("{error:?}")))?
        != original_main
    {
        return Err(DeviceFailure::RecoveryRequired(
            "MiSTer.ini did not retain the original Main selection".into(),
        ));
    }
    match original_main.as_deref() {
        Some("MiSTer_MagiKDev") => {
            wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
            wait_delivery_health(&session, "dev", Duration::from_secs(10))
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
        }
        Some("MiSTer_MagiK") => {
            wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
            wait_delivery_health(&session, "public", Duration::from_secs(10))
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
        }
        None | Some("MiSTer") => {}
        Some(_) => unreachable!("validated alpha host Main selection"),
    }
    Ok("host-mode=restored".into())
}

fn alpha_restore_failure(primary: DeviceFailure, restore: DeviceFailure) -> DeviceFailure {
    DeviceFailure::RecoveryRequired(format!(
        "alpha activation failed: {primary:?}; host-mode restore failed: {restore:?}"
    ))
}

fn require_delivery_sha256(value: &str) -> std::result::Result<(), DeviceFailure> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(DeviceFailure::InvalidRequest(
            "expected SHA-256 is invalid".into(),
        ))
    }
}

fn fetch_verified_development_manager(
    config: &NativeDeviceConfig,
    local: &Path,
    expected_sha256: &str,
) -> std::result::Result<String, DeviceFailure> {
    require_delivery_sha256(expected_sha256)?;
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    exec_checked(
        &session,
        "development platform verify before manager fetch",
        &installed_platform_verify_command(Layout::Development),
    )
    .map_err(device_failure)?;
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or_else(|| DeviceFailure::ArtifactMismatch("development manifest is missing".into()))?;
    if !manifest_has_manager(&manifest, expected_sha256) {
        return Err(DeviceFailure::ArtifactMismatch(
            "installed manager identity does not match the requested artifact".into(),
        ));
    }
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent).map_err(device_failure)?;
    }
    if let Err(error) = get(
        &session,
        "/media/fat/mister-magik-dev/mister-magik-manager",
        local,
    ) {
        let _ = fs::remove_file(local);
        return Err(device_failure(error));
    }
    if file_sha256(local.to_path_buf()).map_err(device_failure)? != expected_sha256 {
        let _ = fs::remove_file(local);
        return Err(DeviceFailure::ArtifactMismatch(
            "downloaded manager checksum does not match its manifest".into(),
        ));
    }
    Ok(format!("manager_sha256={expected_sha256}"))
}

fn manifest_has_manager(manifest: &str, expected_sha256: &str) -> bool {
    manifest
        .lines()
        .filter_map(|line| line.strip_prefix("manager_sha256="))
        .eq([expected_sha256])
}

fn delivery_reboot_wait(config: &NativeDeviceConfig) -> std::result::Result<(), DeviceFailure> {
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    issue_delivery_reboot(&session).map_err(device_failure)?;
    drop(session);
    if !wait_down_with(&config.connection, 40.0)
        || wait_up_with(&config.connection, 120.0).map_err(device_failure)? != 0
    {
        return Err(DeviceFailure::Unavailable(
            "device did not complete its reboot transition".into(),
        ));
    }
    Ok(())
}

fn one_shot_recovery_preflight_command() -> String {
    shell_sequence([
        "set -eu",
        "test ! -e /tmp/mister-magik/reboot-unstable",
        release_arming_cleanup_command(),
        "sync",
    ])
}

fn one_shot_recovery_reboot_wait(
    config: &NativeDeviceConfig,
) -> std::result::Result<(), DeviceFailure> {
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    exec_checked(
        &session,
        "one-shot recovery preflight",
        &one_shot_recovery_preflight_command(),
    )
    .map_err(device_failure)?;
    issue_reboot(&session, RebootMode::Raw).map_err(device_failure)?;
    drop(session);
    if !wait_down_with(&config.connection, 40.0)
        || wait_up_with(&config.connection, 120.0).map_err(device_failure)? != 0
    {
        return Err(DeviceFailure::Unavailable(
            "device did not complete its one-shot recovery reboot".into(),
        ));
    }
    wait_authenticated_agent_ready(config, Duration::from_secs(30))?;
    verify_delivery_health(config)
}

fn wait_authenticated_agent_ready(
    config: &NativeDeviceConfig,
    timeout: Duration,
) -> std::result::Result<(), DeviceFailure> {
    let started = Instant::now();
    let mut last = String::from("agent did not answer");
    while started.elapsed() < timeout {
        match agent_request_at(&config.agent, "ping", json!({}), Duration::from_millis(500)) {
            Ok(_) => return Ok(()),
            Err(error) => last = error.to_string(),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(DeviceFailure::Unavailable(format!(
        "authenticated device agent did not recover: {last}"
    )))
}

fn verify_delivery_health(config: &NativeDeviceConfig) -> std::result::Result<(), DeviceFailure> {
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
        .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
    wait_delivery_health(&session, "dev", Duration::from_secs(10))
        .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))
}

fn smoke_development_delivery(
    config: &NativeDeviceConfig,
    expected_sha256: &str,
) -> std::result::Result<String, DeviceFailure> {
    require_delivery_sha256(expected_sha256)?;
    let command = delivery_smoke_command("dev", expected_sha256).map_err(device_failure)?;
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    if let Err(error) = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45)) {
        return Err(delivery_smoke_failure(&session, &error.to_string()));
    }
    let smoke = (|| -> Result<String> {
        exec_checked(&session, "delivery smoke", &command)?;
        let (_, _, present_state) = wait_delivery_present_state(&session, Duration::from_secs(10))?;
        match present_state {
            DeliveryPresentState::Latch => {
                exec_checked(
                    &session,
                    "delivery latch health",
                    &delivery_health_command("dev")?,
                )?;
                let capture = request_framebuffer_png_at_when_latched(
                    &config.agent,
                    Duration::from_secs(3),
                )?;
                delivery_smoke_capture_detail(&capture)
            }
            DeliveryPresentState::Compatibility => Ok(
                "artifact=verified process=healthy module=degraded latch=compatibility screen=recognized input=ready scanout=rgb565 capture=deferred-to-compatibility evidence=preserved arming=clear"
                    .to_string(),
            ),
        }
    })();
    smoke.map_err(|error| delivery_smoke_failure(&session, &error.to_string()))
}

fn delivery_smoke_failure(session: &Session, error: &str) -> DeviceFailure {
    let evidence = retain_delivery_smoke_failure_evidence(session, error)
        .map(|path| format!("; evidence={}", path.display()))
        .unwrap_or_else(|capture_error| format!("; evidence_capture_failed={capture_error}"));
    DeviceFailure::Unhealthy(format!("{error}{evidence}"))
}

fn wait_delivery_present_state(
    session: &Session,
    timeout: Duration,
) -> Result<(Value, Option<Value>, DeliveryPresentState)> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        attempts = attempts.saturating_add(1);
        let status = read_launcher_status(session)?;
        let evidence = remote_read(session, LATCH_FAILURE_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        match validate_delivery_present_state(&status, evidence.as_ref()) {
            Ok(present_state) => return Ok((status, evidence, present_state)),
            Err(_) if delivery_status_waiting_for_input(&status) && started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(format!(
                    "{error}; present readiness attempts={attempts} elapsed_ms={}",
                    started.elapsed().as_millis()
                )
                .into());
            }
        }
    }
}

fn delivery_status_waiting_for_input(status: &Value) -> bool {
    status.get("scene").and_then(Value::as_str) == Some("launcher")
        && status.get("present_backend").and_then(Value::as_str) == Some("fpga-vblank-latch-hidden")
        && status.get("present_status").and_then(Value::as_str) == Some("ok")
        && status.get("input_enabled").and_then(Value::as_bool) == Some(false)
}

fn retain_delivery_smoke_failure_evidence(session: &Session, failure: &str) -> Result<PathBuf> {
    let output_dir = PathBuf::from("build/delivery-smoke-failures").join(timestamp());
    fs::create_dir_all(&output_dir)?;
    let processes = exec(session, "ps w", true)
        .map(|output| {
            json!({
                "rc": output.rc,
                "stdout": output.stdout,
                "stderr": output.stderr,
            })
        })
        .unwrap_or_else(|error| json!({"error": error.to_string()}));
    let bundle = json!({
        "schema": "mister-magik-delivery-smoke-failure-v1",
        "failure": failure,
        "captured_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        "status": remote_read(session, SLINT_STATUS_REMOTE),
        "main_status": remote_read(session, MAIN_STATUS_REMOTE),
        "latch_failure": remote_read(session, LATCH_FAILURE_REMOTE),
        "slint_log_tail": tail_remote(session, "/tmp/mister-magik-slint.log", 160)
            .map(|lines| lines.join("\n")),
        "main_log_tail": tail_remote(session, "/tmp/mister-magik-main.log", 120)
            .map(|lines| lines.join("\n")),
        "processes": processes,
    });
    fs::write(
        output_dir.join("evidence.json"),
        serde_json::to_vec_pretty(&bundle)?,
    )?;
    Ok(fs::canonicalize(&output_dir).unwrap_or(output_dir))
}

fn retain_diagnostic_evidence(session: &Session, facts: &Value) -> Result<PathBuf> {
    let output_dir = PathBuf::from("build/agent-diagnostics").join(timestamp());
    fs::create_dir_all(&output_dir)?;
    fs::write(
        output_dir.join("diagnostic-facts.json"),
        serde_json::to_vec_pretty(facts)?,
    )?;
    let processes = exec(session, "ps w", true)
        .map(|output| output.stdout)
        .unwrap_or_else(|error| format!("process capture failed: {error}"));
    fs::write(output_dir.join("processes.txt"), processes)?;

    let mut files = vec![
        (
            "/tmp/mister-magik/events.jsonl".to_string(),
            "events.jsonl".to_string(),
        ),
        (
            "/tmp/mister-magik-slint.log".to_string(),
            "slint.log".to_string(),
        ),
        (
            "/tmp/mister-magik-main.log".to_string(),
            "main.log".to_string(),
        ),
        (
            "/tmp/mister-magik/status.json".to_string(),
            "slint-status.json".to_string(),
        ),
        (
            "/tmp/mister-magik/main-status.json".to_string(),
            "main-status.json".to_string(),
        ),
        (
            "/tmp/mister-magik-boot-analytics.tsv".to_string(),
            "boot-analytics.tsv".to_string(),
        ),
        (
            "/tmp/mister-magik/latch-failure.json".to_string(),
            "latch-failure.json".to_string(),
        ),
        (
            "/media/fat/mister-magik-dev/launcher.env".to_string(),
            "launcher.env".to_string(),
        ),
        (
            "/media/fat/mister-magik-dev/platform-v3.manifest".to_string(),
            "platform-v3.manifest".to_string(),
        ),
    ];
    for cycle in 1..=LAUNCH_RETURN_CYCLES {
        files.extend([
            (
                format!("/tmp/mister-magik/launch-return-profile/cycle-{cycle}.svg"),
                format!("cycle-{cycle}-flamegraph.svg"),
            ),
            (
                format!("/tmp/mister-magik/launch-return-profile/cycle-{cycle}.folded"),
                format!("cycle-{cycle}-folded.txt"),
            ),
            (
                format!("/tmp/mister-magik/launch-return-profile/cycle-{cycle}-frames.tsv"),
                format!("cycle-{cycle}-frames.tsv"),
            ),
        ]);
    }
    if let Some(path) = facts
        .get("last_crash_report")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty() && is_safe_crash_report_path(path))
    {
        files.push((path.to_string(), "latest-crash-report.json".to_string()));
    }
    for (remote, local) in files {
        if let Err(error) = get(session, &remote, &output_dir.join(&local)) {
            fs::write(
                output_dir.join(format!("{local}.missing")),
                format!("remote={remote}\nerror={error}\n"),
            )?;
        }
    }
    Ok(fs::canonicalize(&output_dir).unwrap_or(output_dir))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryPresentState {
    Latch,
    Compatibility,
}

fn validate_delivery_present_state(
    status: &Value,
    latch_failure: Option<&Value>,
) -> Result<DeliveryPresentState> {
    let field = |name: &str| {
        status
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("delivery status is missing {name}"))
    };
    let screen = field("screen")?;
    let effective_view = field("effective_view")?;
    let return_screen = field("return_screen")?;
    let present_backend = field("present_backend")?;
    let present_status = field("present_status")?;
    let launch_state = field("launch_state")?;
    if field("scene")? != "launcher" {
        return Err("delivery status is not the launcher scene".into());
    }
    let input_enabled = status
        .get("input_enabled")
        .and_then(Value::as_bool)
        .ok_or("delivery status is missing input_enabled")?;
    if screen != effective_view {
        return Err(format!(
            "delivery status view mismatch screen={screen} effective_view={effective_view}"
        )
        .into());
    }
    if !matches!(
        return_screen,
        "home"
            | "controller"
            | "arcade"
            | "settings"
            | "about"
            | "licenses"
            | "info"
            | "screensaver-settings"
    ) {
        return Err(format!("delivery status has invalid return_screen={return_screen}").into());
    }
    if !matches!(
        effective_view,
        "home"
            | "controller"
            | "arcade"
            | "settings"
            | "about"
            | "licenses"
            | "info"
            | "screensaver-settings"
            | "screensaver"
            | "compatibility"
            | "launching"
    ) {
        return Err(format!("delivery status has invalid effective_view={effective_view}").into());
    }
    if launch_state != "idle" {
        return Err(
            format!("delivery status is not interactive launch_state={launch_state}").into(),
        );
    }
    match (present_backend, present_status) {
        ("fpga-vblank-latch-hidden", "ok") => {
            if effective_view == "compatibility" {
                return Err("latch backend cannot expose the compatibility view".into());
            }
            if !input_enabled {
                return Err("latch delivery input is not enabled".into());
            }
            Ok(DeliveryPresentState::Latch)
        }
        ("compatibility-fb0", "compatibility") => {
            let compatibility_prompt_visible = status
                .get("compatibility_prompt_visible")
                .and_then(Value::as_bool)
                .ok_or("compatibility delivery status is missing compatibility_prompt_visible")?;
            let recovery_state = validate_terminal_compatibility_evidence(
                latch_failure.ok_or("compatibility delivery is missing latch failure evidence")?,
            )?;
            match recovery_state {
                "compatibility-prompt"
                    if effective_view == "compatibility" && compatibility_prompt_visible => {}
                "continued-compatibility"
                    if effective_view != "compatibility"
                        && !compatibility_prompt_visible
                        && input_enabled => {}
                _ => {
                    return Err(format!(
                        "compatibility delivery interaction is inconsistent recovery_state={recovery_state} effective_view={effective_view} prompt_visible={compatibility_prompt_visible} input_enabled={input_enabled}"
                    )
                    .into());
                }
            }
            Ok(DeliveryPresentState::Compatibility)
        }
        _ => Err(format!(
            "delivery status has unsupported presenter backend={present_backend} status={present_status}"
        )
        .into()),
    }
}

fn validate_terminal_compatibility_evidence(evidence: &Value) -> Result<&str> {
    if !matches!(
        evidence.get("schema").and_then(Value::as_str),
        Some("mister-magik-latch-failure-v2" | "mister-magik-latch-failure-v3")
    ) {
        return Err("compatibility delivery has unsupported latch evidence schema".into());
    }
    for field in [
        "state",
        "stage",
        "reason",
        "detail",
        "latest_state",
        "latest_stage",
        "latest_reason",
        "latest_detail",
        "latest_result",
        "recovery_state",
    ] {
        if evidence
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(format!("compatibility delivery evidence is missing {field}").into());
        }
    }
    let recovery_state = evidence["recovery_state"].as_str().unwrap_or_default();
    if !matches!(
        recovery_state,
        "compatibility-prompt" | "continued-compatibility"
    ) {
        return Err(format!(
            "compatibility delivery recovery is not terminal state={recovery_state}"
        )
        .into());
    }
    let latest_result = evidence["latest_result"].as_str().unwrap_or_default();
    if !matches!(latest_result, "not-attempted" | "failure") {
        return Err(format!(
            "compatibility delivery has inconsistent latest_result={latest_result}"
        )
        .into());
    }
    let attempt_count = evidence
        .get("attempt_count")
        .and_then(Value::as_u64)
        .ok_or("compatibility delivery evidence is missing attempt_count")?;
    if (latest_result == "not-attempted" && attempt_count != 0)
        || (latest_result == "failure" && attempt_count == 0)
    {
        return Err(format!(
            "compatibility delivery has inconsistent retry evidence latest_result={latest_result} attempt_count={attempt_count}"
        )
        .into());
    }
    Ok(recovery_state)
}

trait CoherentDeliveryActions {
    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn deploy(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn activate(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn reboot(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn smoke(&mut self) -> std::result::Result<String, DeviceFailure>;
    fn commit(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn rollback(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn health(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn interrupted(&self) -> bool {
        false
    }
}

fn run_coherent_delivery(
    actions: &mut dyn CoherentDeliveryActions,
    reboots: bool,
) -> std::result::Result<String, DeviceFailure> {
    actions.snapshot()?;
    let delivery = (|| {
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        actions.deploy()?;
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        actions.activate()?;
        if reboots {
            actions.reboot()?;
        }
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        let detail = actions.smoke()?;
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        Ok(detail)
    })();
    match delivery {
        Ok(detail) => actions.commit().map(|()| detail).map_err(|error| {
            DeviceFailure::RecoveryRequired(format!(
                "delivery is healthy but commit cleanup failed ({error:?})"
            ))
        }),
        Err(delivery_error) => {
            let rollback = actions
                .rollback()
                .and_then(|()| if reboots { actions.reboot() } else { Ok(()) })
                .and_then(|()| actions.health());
            match rollback {
                Ok(()) => Err(delivery_error),
                Err(error) => Err(DeviceFailure::RecoveryRequired(format!(
                    "delivery failed ({delivery_error:?}); rollback failed ({error:?})"
                ))),
            }
        }
    }
}

fn restore_and_resume(
    restore: impl FnOnce() -> std::result::Result<(), DeviceFailure>,
    resume: impl FnOnce() -> std::result::Result<(), DeviceFailure>,
) -> std::result::Result<(), DeviceFailure> {
    let restore = restore();
    let resume = resume();
    restore.and(resume)
}

struct RuntimeDeliveryActions<'a> {
    config: &'a NativeDeviceConfig,
    session: &'a Session,
    local: &'a Path,
    remote: &'a str,
    manifest_local: &'a Path,
    manifest_remote: &'a str,
    expected_sha256: &'a str,
}

impl CoherentDeliveryActions for RuntimeDeliveryActions<'_> {
    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
        exec_checked(
            self.session,
            "runtime bundle snapshot",
            &format!(
                "set -eu; cp -p {0} {0}.delivery-rollback.tmp; mv -f {0}.delivery-rollback.tmp {0}.delivery-rollback; cp -p {1} {1}.delivery-rollback.tmp; mv -f {1}.delivery-rollback.tmp {1}.delivery-rollback; sync",
                sh(self.remote),
                sh(self.manifest_remote)
            ),
        )
        .map_err(device_failure)
    }

    fn deploy(&mut self) -> std::result::Result<(), DeviceFailure> {
        deploy_magik_bundle(
            self.session,
            self.local,
            self.remote,
            self.manifest_local,
            self.manifest_remote,
            self.expected_sha256,
        )
        .map_err(device_failure)
    }

    fn activate(&mut self) -> std::result::Result<(), DeviceFailure> {
        Ok(())
    }

    fn reboot(&mut self) -> std::result::Result<(), DeviceFailure> {
        delivery_reboot_wait(self.config)
    }

    fn smoke(&mut self) -> std::result::Result<String, DeviceFailure> {
        smoke_development_delivery(self.config, self.expected_sha256)
    }

    fn commit(&mut self) -> std::result::Result<(), DeviceFailure> {
        exec_checked(
            self.session,
            "runtime bundle commit",
            &format!(
                "rm -f {0}.delivery-rollback {1}.delivery-rollback; sync",
                sh(self.remote),
                sh(self.manifest_remote)
            ),
        )
        .map_err(device_failure)
    }

    fn rollback(&mut self) -> std::result::Result<(), DeviceFailure> {
        exec_checked(
            self.session,
            "runtime bundle suspend for rollback",
            &acknowledged_main_command("mister_magik_suspend"),
        )
        .map_err(device_failure)?;
        restore_and_resume(
            || {
                exec_checked(
                    self.session,
                    "runtime bundle rollback",
                    &format!(
                        "set -eu; test -f {0}.delivery-rollback; test -f {1}.delivery-rollback; mv -f {0}.delivery-rollback {0}; chmod 755 {0}; mv -f {1}.delivery-rollback {1}; sync",
                        sh(self.remote),
                        sh(self.manifest_remote)
                    ),
                )
                .map_err(device_failure)
            },
            || {
                exec_checked(
                    self.session,
                    "runtime bundle resume after rollback",
                    &acknowledged_main_command("mister_magik_resume"),
                )
                .map_err(device_failure)
            },
        )
    }

    fn health(&mut self) -> std::result::Result<(), DeviceFailure> {
        verify_delivery_health(self.config)
    }
}

fn deliver_runtime_transaction(
    config: &NativeDeviceConfig,
    local: &Path,
    remote: &str,
    manifest_local: &Path,
    manifest_remote: &str,
    expected_sha256: &str,
) -> std::result::Result<String, DeviceFailure> {
    require_delivery_sha256(expected_sha256)?;
    validate_delivery_remote(remote).map_err(device_failure)?;
    validate_runtime_manifest_remote(manifest_remote).map_err(device_failure)?;
    MagikDeployTransaction::validate_bundle(
        local,
        remote,
        manifest_local,
        manifest_remote,
        expected_sha256,
    )
    .map_err(device_failure)?;
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    run_coherent_delivery(
        &mut RuntimeDeliveryActions {
            config,
            session: &session,
            local,
            remote,
            manifest_local,
            manifest_remote,
            expected_sha256,
        },
        false,
    )
}

struct PlatformDeliveryActions<'a> {
    config: &'a NativeDeviceConfig,
    session: &'a Session,
    transaction: &'a PlatformDeployTransaction,
    expected_sha256: &'a str,
    recovery_reboot_required: bool,
    recovery_reboot_used: bool,
}

impl CoherentDeliveryActions for PlatformDeliveryActions<'_> {
    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
        let reconciliation = exec_checked_output(
            self.session,
            "reconcile interrupted local Main transaction before platform delivery",
            &local_main_reconcile_script(),
        )
        .map_err(device_failure)?;
        self.recovery_reboot_required =
            local_main_reconcile_requires_recovery(&reconciliation.stdout);
        exec_checked(
            self.session,
            "platform snapshot",
            &platform_snapshot_script(),
        )
        .map_err(device_failure)
    }

    fn deploy(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.transaction
            .run(self.session)
            .map(|_| ())
            .map_err(device_failure)
    }

    fn activate(&mut self) -> std::result::Result<(), DeviceFailure> {
        edit_remote_ini(
            self.session,
            IniEdit::SelectMain("MiSTer_MagiKDev".into()),
            false,
        )
        .map_err(device_failure)
    }

    fn reboot(&mut self) -> std::result::Result<(), DeviceFailure> {
        if !self.recovery_reboot_required {
            return delivery_reboot_wait(self.config);
        }
        if self.recovery_reboot_used {
            return Err(DeviceFailure::RecoveryRequired(
                "canonical delivery already used its local-Main recovery reboot".into(),
            ));
        }
        self.recovery_reboot_used = true;
        let reboot = one_shot_recovery_reboot_wait(self.config);
        if reboot.is_ok() {
            self.recovery_reboot_required = false;
        }
        reboot
    }

    fn smoke(&mut self) -> std::result::Result<String, DeviceFailure> {
        smoke_development_delivery(self.config, self.expected_sha256)
    }

    fn commit(&mut self) -> std::result::Result<(), DeviceFailure> {
        let session = connect_with(&self.config.connection, 10).map_err(device_failure)?;
        exec_checked(&session, "platform commit", &platform_cleanup_script())
            .map_err(device_failure)
    }

    fn rollback(&mut self) -> std::result::Result<(), DeviceFailure> {
        let session = connect_with(&self.config.connection, 10).map_err(device_failure)?;
        exec_checked(&session, "platform rollback", &platform_rollback_script())
            .map_err(device_failure)
    }

    fn health(&mut self) -> std::result::Result<(), DeviceFailure> {
        verify_delivery_health(self.config)
    }
}

fn deliver_platform_transaction(
    config: &NativeDeviceConfig,
    stage: &Path,
    expected_sha256: &str,
) -> std::result::Result<String, DeviceFailure> {
    require_delivery_sha256(expected_sha256)?;
    let transaction = PlatformDeployTransaction::validate(stage).map_err(device_failure)?;
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    run_coherent_delivery(
        &mut PlatformDeliveryActions {
            config,
            session: &session,
            transaction: &transaction,
            expected_sha256,
            recovery_reboot_required: false,
            recovery_reboot_used: false,
        },
        true,
    )
}

const LOCAL_MAIN_REMOTE: &str = "/media/fat/MiSTer_MagiKDev";
const LOCAL_MAIN_MANIFEST_REMOTE: &str = "/media/fat/mister-magik-dev/platform-v3.manifest";
const LOCAL_MAIN_TRANSACTION_REMOTE: &str = "/media/fat/mister-magik-dev/local-main.delivery-state";

static LOCAL_MAIN_DELIVERY_INTERRUPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

extern "C" fn local_main_delivery_interrupt_handler(_: libc::c_int) {
    LOCAL_MAIN_DELIVERY_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

struct LocalMainDeliverySignalGuard([(libc::c_int, libc::sighandler_t); 3]);

impl LocalMainDeliverySignalGuard {
    fn install() -> Self {
        LOCAL_MAIN_DELIVERY_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
        Self([libc::SIGHUP, libc::SIGINT, libc::SIGTERM].map(|signal| {
            let previous = unsafe {
                libc::signal(
                    signal,
                    local_main_delivery_interrupt_handler as *const () as libc::sighandler_t,
                )
            };
            (signal, previous)
        }))
    }
}

impl Drop for LocalMainDeliverySignalGuard {
    fn drop(&mut self) {
        for (signal, previous) in self.0 {
            unsafe {
                libc::signal(signal, previous);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalMainActivation {
    LinuxReboot,
    SupervisedReload { pid: u64, generation: u64 },
}

fn local_main_activation(status: Option<&Value>) -> LocalMainActivation {
    let Some(status) = status else {
        return LocalMainActivation::LinuxReboot;
    };
    let supported = status
        .get("local_main_reload_supported")
        .and_then(Value::as_bool)
        == Some(true);
    let active = status.get("launcher_state").and_then(Value::as_str) == Some("LauncherActive");
    let dev = status.get("executable_path").and_then(Value::as_str) == Some(LOCAL_MAIN_REMOTE);
    let pid = status.get("pid").and_then(Value::as_u64);
    let generation = status.get("main_generation").and_then(Value::as_u64);
    match (supported && active && dev, pid, generation) {
        (true, Some(pid), Some(generation)) if pid != 0 && generation != 0 => {
            LocalMainActivation::SupervisedReload { pid, generation }
        }
        _ => LocalMainActivation::LinuxReboot,
    }
}

struct LocalMainDeliveryActions<'a> {
    config: &'a NativeDeviceConfig,
    local: &'a Path,
    manifest_local: &'a Path,
    expected_main_sha256: &'a str,
    expected_gui_sha256: &'a str,
    activation: LocalMainActivation,
    installed_manifest: Option<BTreeMap<String, String>>,
    rolling_back: bool,
    recovery_reboot_used: bool,
}

impl LocalMainDeliveryActions<'_> {
    fn connect(&self) -> std::result::Result<Session, DeviceFailure> {
        connect_with(&self.config.connection, 10).map_err(device_failure)
    }

    fn reload(&self, previous_pid: u64, previous_generation: u64) -> Result<()> {
        let session = connect_with(&self.config.connection, 10)?;
        exec_checked(
            &session,
            "local Main supervised reload",
            &acknowledged_main_command("mister_magik_reload_main"),
        )?;
        let started = Instant::now();
        let mut last_status = Value::Null;
        while started.elapsed() < Duration::from_secs(45) {
            if let Some(text) = remote_read(&session, MAIN_STATUS_REMOTE)
                && let Ok(status) = serde_json::from_str::<Value>(&text)
            {
                let pid = status.get("pid").and_then(Value::as_u64).unwrap_or(0);
                let generation = status
                    .get("main_generation")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if pid != 0
                    && generation != 0
                    && pid != previous_pid
                    && generation != previous_generation
                {
                    return Ok(());
                }
                last_status = status;
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "local Main reload did not produce a new process generation; last_status={last_status}"
        )
        .into())
    }

    fn recovery_reboot(
        &mut self,
        reason: impl std::fmt::Display,
    ) -> std::result::Result<(), DeviceFailure> {
        if self.recovery_reboot_used {
            return Err(DeviceFailure::RecoveryRequired(format!(
                "local Main recovery reboot already used ({reason})"
            )));
        }
        self.recovery_reboot_used = true;
        eprintln!("local Main rollback requires one bounded recovery reboot ({reason})");
        one_shot_recovery_reboot_wait(self.config)
    }

    fn activate_installed_main(&mut self) -> std::result::Result<(), DeviceFailure> {
        match self.activation {
            LocalMainActivation::LinuxReboot if self.rolling_back => {
                self.recovery_reboot("supervised Main reload is unavailable")
            }
            LocalMainActivation::LinuxReboot => delivery_reboot_wait(self.config),
            LocalMainActivation::SupervisedReload { pid, generation } => {
                match self.reload(pid, generation) {
                    Ok(()) => Ok(()),
                    Err(error) if self.rolling_back => self.recovery_reboot(error),
                    Err(error) => Err(device_failure(error)),
                }
            }
        }
    }
}

impl CoherentDeliveryActions for LocalMainDeliveryActions<'_> {
    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
        let session = self.connect()?;
        exec_checked(
            &session,
            "reconcile interrupted local Main transaction",
            &local_main_reconcile_script(),
        )
        .map_err(device_failure)?;
        exec_checked(
            &session,
            "installed Dev platform verification before local Main delivery",
            &installed_platform_verify_command(Layout::Development),
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        let status = remote_read(&session, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        self.activation = local_main_activation(status.as_ref());
        let installed_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
            .ok_or_else(|| DeviceFailure::ArtifactMismatch("Dev manifest is missing".into()))?;
        self.installed_manifest = Some(
            parse_local_main_manifest_text(&installed_manifest)
                .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?,
        );
        exec_checked(
            &session,
            "local Main snapshot",
            &local_main_snapshot_script(),
        )
        .map_err(device_failure)
    }

    fn deploy(&mut self) -> std::result::Result<(), DeviceFailure> {
        let candidate = parse_local_main_manifest(self.manifest_local).map_err(device_failure)?;
        validate_local_main_overlay_preserves_installed(
            self.installed_manifest.as_ref().ok_or_else(|| {
                DeviceFailure::OperationFailed("local Main snapshot identity is missing".into())
            })?,
            &candidate,
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        let session = self.connect()?;
        put(&session, self.local, &format!("{LOCAL_MAIN_REMOTE}.upload"))
            .map_err(device_failure)?;
        put(
            &session,
            self.manifest_local,
            &format!("{LOCAL_MAIN_MANIFEST_REMOTE}.upload"),
        )
        .map_err(device_failure)?;
        exec_checked(
            &session,
            "local Main activation",
            &local_main_swap_script(
                self.expected_main_sha256,
                &file_sha256(self.manifest_local.to_path_buf()).map_err(device_failure)?,
            ),
        )
        .map_err(device_failure)
    }

    fn activate(&mut self) -> std::result::Result<(), DeviceFailure> {
        Ok(())
    }

    fn reboot(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.activate_installed_main()
    }

    fn smoke(&mut self) -> std::result::Result<String, DeviceFailure> {
        let mut detail = smoke_development_delivery(self.config, self.expected_gui_sha256)?;
        let session = self.connect()?;
        exec_checked(
            &session,
            "local Main installed platform verification",
            &installed_platform_verify_command(Layout::Development),
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        exec_checked(
            &session,
            "local Main running process identity",
            &local_main_process_identity_command(self.expected_main_sha256),
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        detail.push_str(&format!(" main_sha256={}", self.expected_main_sha256));
        Ok(detail)
    }

    fn commit(&mut self) -> std::result::Result<(), DeviceFailure> {
        let session = self.connect()?;
        exec_checked(&session, "local Main commit", &local_main_cleanup_script())
            .map_err(device_failure)
    }

    fn rollback(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.rolling_back = true;
        let session = self.connect()?;
        let status = remote_read(&session, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        self.activation = local_main_activation(status.as_ref());
        exec_checked(
            &session,
            "local Main rollback",
            &local_main_rollback_script(),
        )
        .map_err(device_failure)
    }

    fn health(&mut self) -> std::result::Result<(), DeviceFailure> {
        let session = self.connect()?;
        exec_checked(
            &session,
            "restored Dev platform verification",
            &installed_platform_verify_command(Layout::Development),
        )
        .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
        verify_delivery_health(self.config)?;
        if self.rolling_back {
            exec_checked(
                &session,
                "local Main rollback commit",
                &local_main_rollback_cleanup_script(),
            )
            .map_err(device_failure)?;
        }
        Ok(())
    }

    fn interrupted(&self) -> bool {
        LOCAL_MAIN_DELIVERY_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn validate_local_main_bundle_identity(
    local: &Path,
    manifest_local: &Path,
    expected_main_sha256: &str,
    expected_gui_sha256: &str,
) -> Result<()> {
    if !local.is_file() || !manifest_local.is_file() {
        return Err("local Main delivery requires a Main artifact and manifest".into());
    }
    if file_sha256(local.to_path_buf())? != expected_main_sha256 {
        return Err("local Main artifact hash does not match the requested identity".into());
    }
    let fields = parse_local_main_manifest(manifest_local)?;
    for (field, expected) in [
        ("main_sha256", expected_main_sha256),
        ("gui_sha256", expected_gui_sha256),
        ("main_path", LOCAL_MAIN_REMOTE),
        ("gui_path", "/media/fat/mister-magik-dev/mister-magik-fb"),
    ] {
        if fields.get(field).map(String::as_str) != Some(expected) {
            return Err(format!("local Main manifest field {field} is not canonical").into());
        }
    }
    Ok(())
}

fn parse_local_main_manifest(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = fs::read_to_string(path)?;
    parse_local_main_manifest_text(&text)
}

fn parse_local_main_manifest_text(text: &str) -> Result<BTreeMap<String, String>> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or("local Main manifest contains a malformed line")?;
        if value.is_empty()
            || !RUNTIME_MANIFEST_FIELDS.contains(&key)
            || fields.insert(key.into(), value.into()).is_some()
        {
            return Err(format!("local Main manifest has an invalid field: {key}").into());
        }
    }
    if fields.len() != RUNTIME_MANIFEST_FIELDS.len()
        || RUNTIME_MANIFEST_FIELDS
            .iter()
            .any(|field| !fields.contains_key(*field))
    {
        return Err("local Main manifest does not have the exact canonical field set".into());
    }
    for field in [
        "platform_bundle_id",
        "qualification_candidate_id",
        "main_sha256",
        "gui_sha256",
        "manager_sha256",
        "scanout_module_sha256",
        "scanout_metadata_sha256",
        "latch_rbf_sha256",
        "latch_metadata_sha256",
        "platform_contract_sha256",
    ] {
        require_local_main_hex(field, &fields[field], 64)?;
    }
    for field in ["main_revision", "magik_revision", "menu_revision"] {
        require_local_main_hex(field, &fields[field], 40)?;
    }
    if fields["format"] != "mister-magik-platform-v3"
        || fields["main_path"] != LOCAL_MAIN_REMOTE
        || fields["gui_path"] != "/media/fat/mister-magik-dev/mister-magik-fb"
    {
        return Err("local Main manifest has a non-Dev platform identity".into());
    }
    if fields["qualification_candidate_id"] != local_main_candidate_id(&fields) {
        return Err("local Main manifest candidate identity is inconsistent".into());
    }
    Ok(fields)
}

fn validate_local_main_overlay_preserves_installed(
    installed: &BTreeMap<String, String>,
    candidate: &BTreeMap<String, String>,
) -> Result<()> {
    for field in RUNTIME_MANIFEST_FIELDS {
        if matches!(
            *field,
            "main_sha256" | "main_revision" | "qualification_candidate_id"
        ) {
            continue;
        }
        if candidate.get(*field) != installed.get(*field) {
            return Err(
                format!("local Main overlay changed protected platform field {field}").into(),
            );
        }
    }
    Ok(())
}

fn local_main_candidate_id(fields: &BTreeMap<String, String>) -> String {
    let mut hash = Sha256::new();
    for field in RUNTIME_MANIFEST_FIELDS {
        if *field == "qualification_candidate_id" {
            continue;
        }
        hash.update(field.as_bytes());
        hash.update(b"=");
        hash.update(fields[*field].as_bytes());
        hash.update(b"\n");
    }
    encode_hex(&hash.finalize())
}

fn require_local_main_hex(name: &str, value: &str, length: usize) -> Result<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!("local Main manifest field {name} is not lowercase hex").into())
    }
}

fn local_main_snapshot_script() -> String {
    let safety = platform_safety_script();
    format!(
        "set -eu; {safety}; test ! -e {transaction}; test ! -e {main}.delivery-rollback; test ! -e {manifest}.delivery-rollback; rm -f {main}.upload {manifest}.upload; cp -p {main} {main}.delivery-rollback.tmp; mv -f {main}.delivery-rollback.tmp {main}.delivery-rollback; cp -p {manifest} {manifest}.delivery-rollback.tmp; mv -f {manifest}.delivery-rollback.tmp {manifest}.delivery-rollback; printf 'snapshot\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_reconcile_script() -> String {
    format!(
        "set -eu; state=none; test ! -f {transaction} || state=$(cat {transaction}); if test \"$state\" = validated; then rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload {transaction}; sync; printf 'local-main-reconcile=validated\\n'; elif test -f {transaction}; then test -f {main}.delivery-rollback; test -f {manifest}.delivery-rollback; cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; sync; rm -f {transaction}; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; sync; printf 'local-main-reconcile=%s\\n' \"$state\"; elif test -f {main}.delivery-rollback && test -f {manifest}.delivery-rollback; then cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; sync; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; sync; printf 'local-main-reconcile=orphan\\n'; else rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; printf 'local-main-reconcile=none\\n'; fi",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_reconcile_requires_recovery(output: &str) -> bool {
    output.lines().any(|line| {
        line.strip_prefix("local-main-reconcile=")
            .is_some_and(|state| !matches!(state, "none" | "validated" | "snapshot" | "orphan"))
    })
}

fn local_main_swap_script(expected_main_sha256: &str, expected_manifest_sha256: &str) -> String {
    format!(
        "set -eu; test -f {transaction}; test -f {main}.delivery-rollback; test -f {manifest}.delivery-rollback; test \"$(sha256sum {main}.upload | awk '{{print $1}}')\" = {main_hash}; test \"$(sha256sum {manifest}.upload | awk '{{print $1}}')\" = {manifest_hash}; printf 'activating\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; sync; mv -f {main}.upload {main}; chmod 755 {main}; mv -f {manifest}.upload {manifest}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        main_hash = sh(expected_main_sha256),
        manifest_hash = sh(expected_manifest_sha256),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_rollback_script() -> String {
    format!(
        "set -eu; test -f {transaction}; test -f {main}.delivery-rollback; test -f {manifest}.delivery-rollback; cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; printf 'rolled-back\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; rm -f {main}.upload {manifest}.upload; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_cleanup_script() -> String {
    format!(
        "set -eu; test -f {transaction}; printf 'validated\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; sync; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; rm -f {transaction}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_rollback_cleanup_script() -> String {
    format!(
        "set -eu; test \"$(cat {transaction})\" = rolled-back; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; rm -f {transaction}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(LOCAL_MAIN_TRANSACTION_REMOTE),
    )
}

fn local_main_process_identity_command(expected_main_sha256: &str) -> String {
    format!(
        "set -eu; set -- $(pidof MiSTer_MagiKDev); test \"$#\" -eq 1; test \"$(readlink /proc/$1/exe)\" = {main}; test \"$(sha256sum /proc/$1/exe | awk '{{print $1}}')\" = {expected}",
        main = sh(LOCAL_MAIN_REMOTE),
        expected = sh(expected_main_sha256),
    )
}

fn deliver_local_main_transaction(
    config: &NativeDeviceConfig,
    local: &Path,
    manifest_local: &Path,
    expected_main_sha256: &str,
    expected_gui_sha256: &str,
) -> std::result::Result<String, DeviceFailure> {
    let _signal_guard = LocalMainDeliverySignalGuard::install();
    require_delivery_sha256(expected_main_sha256)?;
    require_delivery_sha256(expected_gui_sha256)?;
    validate_local_main_bundle_identity(
        local,
        manifest_local,
        expected_main_sha256,
        expected_gui_sha256,
    )
    .map_err(device_failure)?;
    run_coherent_delivery(
        &mut LocalMainDeliveryActions {
            config,
            local,
            manifest_local,
            expected_main_sha256,
            expected_gui_sha256,
            activation: LocalMainActivation::LinuxReboot,
            installed_manifest: None,
            rolling_back: false,
            recovery_reboot_used: false,
        },
        true,
    )
}

fn device_failure(error: impl std::fmt::Display) -> DeviceFailure {
    let detail = error.to_string();
    let lower = detail.to_ascii_lowercase();
    if lower.contains("local-network access denied") {
        DeviceFailure::AccessDenied(detail)
    } else if lower.contains("authentication") || lower.contains("permission denied") {
        DeviceFailure::Authentication(detail)
    } else if lower.contains("connect")
        || lower.contains("timeout")
        || lower.contains("unreachable")
    {
        DeviceFailure::Unavailable(detail)
    } else {
        DeviceFailure::OperationFailed(detail)
    }
}

pub fn run_cli() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        return Ok(());
    }
    let action = args.remove(0);
    if action == "--capture-buffer" {
        validate_capture_buffer_args(&args)?;
    }
    reject_retired_platform_command(&action)?;
    if action_uses_device(&action) {
        if env::var_os(RESOLVED_DEVICE_CHILD).is_none() {
            let device = discovery::resolve()?;
            let status = Command::new(env::current_exe()?)
                .args(env::args_os().skip(1))
                .env("MISTER_IP", device.address.to_string())
                .env("MISTER_DEVICE_ID", &device.id)
                .env(RESOLVED_DEVICE_CHILD, "1")
                .status()?;
            if status.success() {
                return Ok(());
            }
            std::process::exit(status.code().unwrap_or(1));
        }
        if std::env::var_os("MISTER_SKIP_AGENT_BOOTSTRAP").is_none() {
            bootstrap_agent()?;
        }
    }
    match action.as_str() {
        "--capture-buffer" => capture_buffer(&args)?,
        "arming-status" => arming_status()?,
        "core-list" => core_list()?,
        "mode" => mode_cli(&args)?,
        "scene" => scene_cli(&args)?,
        "display-mode" => display_mode_cli(&args)?,
        "display-matrix" => display_matrix_cli(&args)?,
        "crt" => crt_qualification::run(&args)?,
        "connected" => println!("connected"),
        "get" => {
            if args.len() < 2 {
                return Err("get needs <remote> <local>".into());
            }
            let sess = connect(10)?;
            get(&sess, &args[0], Path::new(&args[1]))?;
            println!("get {} -> {}", args[0], args[1]);
        }
        "db" | "library-db" => {
            return Err(
                "mister db was retired with Catalog V2; use mister catalog to validate Catalog V3"
                    .into(),
            );
        }
        "catalog" => {
            let sess = connect(10)?;
            run_catalog_inspect(&sess, &args)?;
        }
        "wait" => {
            let secs = args.first().and_then(|s| s.parse().ok()).unwrap_or(120.0);
            std::process::exit(wait_up(secs)?);
        }
        "connection-profile" => {
            connection_profile(&args)?;
        }
        "media-check" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_check(&sess, &args)?;
        }
        "media-download" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_download(&sess, &args)?;
        }
        "media-bench-download" => {
            if media::media_help_requested(&args) {
                media::media_usage();
                return Ok(());
            }
            let sess = connect(10)?;
            media::media_bench_download(&sess, &args)?;
        }
        "media-cloudflare-check" => {
            media::media_cloudflare_check(&args)?;
        }
        "launcher-restart" => {
            if launcher_restart_help_requested(&args) {
                launcher_restart_usage();
                return Ok(());
            }
            let options = parse_launcher_restart_args(&args)?;
            let sess = connect(10)?;
            launcher_restart(&sess, &options)?;
        }
        "boot-net-profile" => {
            boot_net_profile(&args)?;
        }
        "boot-tcp-profile" => {
            boot_tcp_profile(&args)?;
        }
        "agent" => {
            agent_cli(&args)?;
        }
        "watch-reboot" => {
            watch_external_reboot(&args)?;
        }
        "reboot" | "reboot-wait" => {
            let mode = take_reboot_mode_flag(&mut args)?;
            let host = host();
            {
                let sess = connect(10)?;
                let issued = issue_reboot(&sess, mode)?;
                println!("reboot issued to {host} ({issued})");
            }
            if action == "reboot-wait" {
                if !wait_down(40.0) {
                    return Err(
                        "reboot-wait did not observe the device go down; refusing to treat the existing SSH session as a reboot"
                            .into(),
                    );
                }
                let secs = args.first().and_then(|s| s.parse().ok()).unwrap_or(120.0);
                std::process::exit(wait_up(secs)?);
            }
        }
        "status" => {
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print_status_summary(&status);
            }
        }
        "delivery-health" => {
            let layout = args.first().map(String::as_str).unwrap_or("dev");
            let command = delivery_health_command(layout)?;
            let sess = connect(10)?;
            exec_checked(&sess, "delivery health", &command)?;
            println!("healthy");
        }
        "doctor" => {
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            let findings = doctor_findings(&status);
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"status": status, "findings": findings}))?
                );
            } else {
                print_status_summary(&status);
                println!("\nDoctor findings");
                for (level, text) in findings {
                    println!("  [{level}] {text}");
                }
            }
        }
        "boot-capture" => {
            let keep_enabled = args.iter().any(|a| a == "--keep-enabled");
            let deploy = args.iter().any(|a| a == "--deploy");
            let settle = option_value(&args, "--settle")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            boot_capture(deploy, keep_enabled, settle)?;
        }
        "display-read" => {
            let unsafe_spi = args.iter().any(|a| a == "--unsafe-spi");
            let json_out = args.iter().any(|a| a == "--json");
            let sess = connect(10)?;
            display_read(&sess, unsafe_spi, json_out)?;
        }
        "ini-repair-boot" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::MagikBoot, dry_run)?;
        }
        "ini-select-main" => {
            let value = args
                .first()
                .ok_or("ini-select-main needs <MiSTer|MiSTer_MagiK|MiSTer_MagiKDev>")?;
            if !matches!(
                value.as_str(),
                "MiSTer" | "MiSTer_MagiK" | "MiSTer_MagiKDev"
            ) {
                return Err("unsupported main selection".into());
            }
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::SelectMain(value.clone()), dry_run)?;
        }
        "inittab-ensure-stock" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            ensure_stock_inittab(&sess, dry_run)?;
        }
        "ini-repair-arcade-video" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::ArcadeVideo, dry_run)?;
        }
        "ini-restore-stock" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::StockBoot, dry_run)?;
        }
        "ini-zaparoo-boot" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let sess = connect(10)?;
            edit_remote_ini(&sess, IniEdit::ZaparooBoot, dry_run)?;
        }
        "ini-edit" => {
            let dry_run = args.last().map(String::as_str) == Some("--dry-run");
            if dry_run {
                args.pop();
            }
            validate_ini_edit_args(&args)?;
            let edit = parse_ini_edit_args(&args)?;
            let sess = connect(10)?;
            edit_remote_ini(&sess, edit, dry_run)?;
        }
        "profile-summary" => {
            let path = args
                .first()
                .ok_or("profile-summary needs <frame-profile.tsv>")?;
            profile_summary(Path::new(path))?;
        }
        "mame-metadata-build" => {
            mame_metadata_build(&args)?;
        }
        "arcade-database-import" => {
            let sqlite = option_value(&args, "--sqlite")
                .ok_or("arcade-database-import needs --sqlite <mame.sqlite3>")?;
            let csv = option_value(&args, "--csv")
                .ok_or("arcade-database-import needs --csv <ArcadeDatabase.csv>")?;
            let source_sha = option_value(&args, "--source-sha")
                .ok_or("arcade-database-import needs --source-sha <commit>")?;
            let summary =
                arcade_database::import(Path::new(&sqlite), Path::new(&csv), &source_sha)?;
            println!("{}", serde_json::to_string(&summary)?);
        }
        "recover" => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            if !dry_run {
                return Err("recover currently supports --dry-run only".into());
            }
            let sess = connect(10)?;
            let status = collect_status(&sess)?;
            println!("Dry-run recovery suggestions");
            for (_, text) in doctor_findings(&status) {
                println!("  - {text}");
            }
            println!("  - Mutating recovery is intentionally not implemented yet.");
        }
        "-h" | "--help" => usage(),
        other => return Err(format!("unknown action: {other}").into()),
    }
    Ok(())
}

const CLI_USAGE: &str = "usage: mister --capture-buffer\n       mister <status|arming-status|mode|scene|display-mode|display-matrix|crt|ini-edit|core-list|catalog|media-check|media-download|agent|reboot-wait|doctor|mame-metadata-build|arcade-database-import> ...\n       mode <status|dev|public|stock>\n       scene <launcher|controller_test|tear_pattern|video_playback|crt_trial> [seconds]\n       display-mode MODE --attended [--keep]\n         MODE: auto|hdmi-1280x720p60|hdmi-1366x768p60|hdmi-1920x1080p60\n               hdmi-1920x1200p60|hdmi-2048x1536p60|hdmi-2560x1440p60\n               crt-240p60|crt-288p50|crt-480p60|crt-576p50\n       display-matrix --attended --out DIRECTORY\n       crt qualify --attended [--out DIRECTORY]\n       crt qualify --restore\n       ini-edit menu <OUTPUT> [--dry-run]\n       OUTPUT: hdmi|auto|crt-240p60|crt-288p50|crt-480p60|crt-576p50\n               1280x720p60|1024x768p60|720x480p60|720x576p50|1280x1024p60\n               800x600p60|640x480p60|1280x720p50|1920x1080p60|1920x1080p50\n               1366x768p60|1024x600p60|1920x1440p60|2048x1536p60\n       2560x1440p60: Mister does not support 1440p\n       ini-edit stock-boot [--dry-run]\n       mame-metadata-build --out <sqlite> [--listxml <xml>|--mame <bin>|--machine-sqlite <sqlite>]\n       arcade-database-import --sqlite <mame.sqlite3> --csv <ArcadeDatabase.csv> --source-sha <commit>\n       operator commands are typed and bounded; direct-reset-no-sync remains experimental and requires a volatile session token";

fn usage() {
    println!("{CLI_USAGE}");
    println!("       crt probe --attended --pattern PATTERN --seconds 20 --out DIRECTORY");
    println!("       display-matrix optional evidence: --usb-video [--screensaver-wait SECONDS]");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisplayMatrixMode {
    id: &'static str,
    output: Option<(u16, u16)>,
    framebuffer: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisplayMatrixReadiness {
    output: (usize, usize),
    framebuffer: (usize, usize),
    frames_before: u64,
    frames_after: u64,
    idle: bool,
}

const DISPLAY_MATRIX_MODES: &[DisplayMatrixMode] = &[
    DisplayMatrixMode {
        id: "auto",
        output: None,
        framebuffer: None,
    },
    DisplayMatrixMode {
        id: "hdmi-1280x720p60",
        output: Some((1280, 720)),
        framebuffer: Some((1280, 720)),
    },
    DisplayMatrixMode {
        id: "hdmi-1366x768p60",
        output: Some((1366, 768)),
        framebuffer: Some((683, 384)),
    },
    DisplayMatrixMode {
        id: "hdmi-1920x1080p60",
        output: Some((1920, 1080)),
        framebuffer: Some((960, 540)),
    },
    DisplayMatrixMode {
        id: "hdmi-1920x1200p60",
        output: Some((1920, 1200)),
        framebuffer: Some((960, 600)),
    },
    DisplayMatrixMode {
        id: "hdmi-2048x1536p60",
        output: Some((2048, 1536)),
        framebuffer: Some((1024, 768)),
    },
    DisplayMatrixMode {
        id: "hdmi-2560x1440p60",
        output: Some((2560, 1440)),
        framebuffer: Some((1280, 720)),
    },
    DisplayMatrixMode {
        id: "crt-240p60",
        output: Some((640, 240)),
        framebuffer: Some((640, 240)),
    },
    DisplayMatrixMode {
        id: "crt-288p50",
        output: Some((640, 288)),
        framebuffer: Some((640, 288)),
    },
    DisplayMatrixMode {
        id: "crt-480p60",
        output: Some((640, 480)),
        framebuffer: Some((640, 480)),
    },
    DisplayMatrixMode {
        id: "crt-576p50",
        output: Some((640, 576)),
        framebuffer: Some((640, 576)),
    },
];

static DISPLAY_MATRIX_INTERRUPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SCREENSAVER_PROFILE_INTERRUPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct DisplayMatrixEvidence {
    usb_video: bool,
    screensaver_wait_secs: Option<u64>,
}

extern "C" fn display_matrix_interrupt_handler(_: libc::c_int) {
    DISPLAY_MATRIX_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

extern "C" fn screensaver_profile_interrupt_handler(_: libc::c_int) {
    SCREENSAVER_PROFILE_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn screensaver_profile_interrupted() -> bool {
    SCREENSAVER_PROFILE_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

struct ScreensaverProfileSignalGuard([(libc::c_int, libc::sighandler_t); 3]);

impl ScreensaverProfileSignalGuard {
    fn install() -> Self {
        SCREENSAVER_PROFILE_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
        Self([libc::SIGHUP, libc::SIGINT, libc::SIGTERM].map(|signal| {
            let previous = unsafe {
                libc::signal(
                    signal,
                    screensaver_profile_interrupt_handler as *const () as libc::sighandler_t,
                )
            };
            (signal, previous)
        }))
    }
}

impl Drop for ScreensaverProfileSignalGuard {
    fn drop(&mut self) {
        for (signal, previous) in self.0 {
            unsafe {
                libc::signal(signal, previous);
            }
        }
    }
}

struct SignalHandlerGuard(libc::sighandler_t);

impl Drop for SignalHandlerGuard {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.0);
        }
    }
}

fn parse_display_mode_args(args: &[String]) -> Result<(DisplayMatrixMode, bool)> {
    if args.len() < 2 || args.len() > 3 || args[1] != "--attended" {
        return Err("usage: mister display-mode MODE --attended [--keep]".into());
    }
    let keep = args.len() == 3 && args[2] == "--keep";
    if args.len() == 3 && !keep {
        return Err("usage: mister display-mode MODE --attended [--keep]".into());
    }
    let mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == args[0])
        .copied()
        .ok_or_else(|| format!("unsupported display mode: {}", args[0]))?;
    Ok((mode, keep))
}

fn display_mode_cli(args: &[String]) -> Result<()> {
    let (mode, keep) = parse_display_mode_args(args)?;
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("display mode changes are attended and require an interactive terminal".into());
    }
    if matches!(mode.id, "crt-480p60" | "crt-576p50") {
        eprintln!(
            "WARNING: this is a 31 kHz CRT/VGA mode. Type 31KHZ only if the connected display supports it:"
        );
        let mut acknowledgement = String::new();
        io::stdin().read_line(&mut acknowledgement)?;
        if acknowledgement.trim() != "31KHZ" {
            return Err("31 kHz display support was not acknowledged; no mode changed".into());
        }
    }
    DISPLAY_MATRIX_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
    let signal_guard = SignalHandlerGuard(unsafe {
        libc::signal(
            libc::SIGINT,
            display_matrix_interrupt_handler as *const () as libc::sighandler_t,
        )
    });
    let session = connect(10)?;
    let original_reply = exec_checked_output(
        &session,
        "query original display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("display mode cannot change while a transaction is pending".into());
    }
    let original_ready = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    exec_checked(
        &session,
        "apply display mode",
        &acknowledged_main_command(&format!(
            "mister_magik_display_apply_headless_v1 mode={}",
            mode.id
        )),
    )?;
    drop(session);
    let mut current_pid = original_ready.launcher_pid;
    let result = (|| -> Result<()> {
        let session = connect(10)?;
        let ready = wait_launcher_ready_after(
            &session,
            current_pid,
            Instant::now(),
            Duration::from_secs(15),
        )?;
        current_pid = ready.launcher_pid;
        let readiness = validate_live_display_mode(&session, mode)?;
        let capture = request_framebuffer_png()?;
        validate_visible_launcher_capture(&capture)?;
        if png_dimensions(&capture.png)?
            != (
                readiness.framebuffer.0 as u32,
                readiness.framebuffer.1 as u32,
            )
        {
            return Err("framebuffer capture geometry does not match display plan".into());
        }
        match display_mode_completion_action(
            keep,
            DISPLAY_MATRIX_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst),
        )? {
            DisplayModeCompletionAction::Confirm => {
                exec_checked(
                    &session,
                    "confirm display mode",
                    &acknowledged_main_command("mister_magik_display_confirm_v1"),
                )?;
                wait_display_transaction_idle(&session, Duration::from_secs(15))?;
                println!("kept {}", mode.id);
            }
            DisplayModeCompletionAction::Rollback => {
                exec_checked(
                    &session,
                    "rollback display mode",
                    &acknowledged_main_command("mister_magik_display_cancel_v1"),
                )?;
                let _ = wait_launcher_ready_after(
                    &session,
                    current_pid,
                    Instant::now(),
                    Duration::from_secs(15),
                )?;
                let restored = exec_checked_output(
                    &session,
                    "verify restored display mode",
                    &acknowledged_main_command("mister_magik_display_get_v1"),
                )?;
                let active = parse_display_reply_active(restored.stdout.trim())?;
                if active != original_mode {
                    return Err(format!(
                        "display mode restored {active}, expected {original_mode}"
                    )
                    .into());
                }
                if let Some(original) = DISPLAY_MATRIX_MODES
                    .iter()
                    .find(|candidate| candidate.id == original_mode)
                    .copied()
                {
                    validate_live_display_mode(&session, original)?;
                }
                println!("verified {} and restored {}", mode.id, original_mode);
            }
        }
        Ok(())
    })();
    let result = if result.is_err() {
        combine_display_mode_result(
            result,
            restore_display_matrix_original(&original_mode, current_pid),
        )
    } else {
        result
    };
    drop(signal_guard);
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayModeCompletionAction {
    Confirm,
    Rollback,
}

fn display_mode_completion_action(
    keep: bool,
    interrupted: bool,
) -> Result<DisplayModeCompletionAction> {
    if interrupted {
        return Err("display mode change interrupted".into());
    }
    Ok(if keep {
        DisplayModeCompletionAction::Confirm
    } else {
        DisplayModeCompletionAction::Rollback
    })
}

fn combine_display_mode_result(primary: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (primary, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(format!("{error}; display rollback failed: {cleanup}").into())
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn validate_live_display_mode(
    session: &Session,
    mode: DisplayMatrixMode,
) -> Result<DisplayMatrixReadiness> {
    let readiness = exec_checked_output(
        session,
        "display mode readiness",
        &release_display_mode_command_for_runtime(),
    )?;
    let readiness = parse_display_matrix_readiness(&readiness.stdout)?;
    validate_display_matrix_geometry(mode, readiness.output, readiness.framebuffer)?;
    if readiness.frames_after <= readiness.frames_before && !readiness.idle {
        return Err("display presentation did not advance".into());
    }
    Ok(readiness)
}

fn wait_display_transaction_idle(session: &Session, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while !display_poll_timed_out(started.elapsed(), timeout) {
        let reply = exec_checked_output(
            session,
            "query display confirmation",
            &acknowledged_main_command("mister_magik_display_get_v1"),
        )?;
        if display_transaction_complete(
            reply.stdout.trim(),
            DISPLAY_MATRIX_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst),
        )? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("display persistence timed out".into())
}

fn display_poll_timed_out(elapsed: Duration, timeout: Duration) -> bool {
    elapsed >= timeout
}

fn display_transaction_complete(reply: &str, interrupted: bool) -> Result<bool> {
    if interrupted {
        return Err("display mode change interrupted".into());
    }
    if reply
        .split_whitespace()
        .any(|field| field == "phase=failed")
    {
        return Err("display persistence failed".into());
    }
    Ok(parse_display_reply_pending(reply)?.is_none())
}

fn display_matrix_cli(args: &[String]) -> Result<()> {
    let (directory, capture_usb_video, screensaver_wait_secs) = parse_display_matrix_args(args)?;
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("display matrix is attended and requires an interactive terminal".into());
    }
    eprintln!(
        "WARNING: this matrix includes 31 kHz CRT/VGA modes. Type 31KHZ only if the connected display supports them:"
    );
    let mut acknowledgement = String::new();
    io::stdin().read_line(&mut acknowledgement)?;
    if acknowledgement.trim() != "31KHZ" {
        return Err("31 kHz display support was not acknowledged; no modes changed".into());
    }
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory)?;
    let directory = fs::canonicalize(directory)?;
    DISPLAY_MATRIX_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
    let session = connect(10)?;
    let original_reply = exec_checked_output(
        &session,
        "query original display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("display matrix cannot start while a display transaction is pending".into());
    }
    let original_ready = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    drop(session);
    let previous_sigint = unsafe {
        libc::signal(
            libc::SIGINT,
            display_matrix_interrupt_handler as *const () as libc::sighandler_t,
        )
    };
    let mut current_pid = original_ready.launcher_pid;
    let mut entries = Vec::new();
    let mut seen_hashes = std::collections::HashSet::new();
    let mut seen_usb_hashes = std::collections::HashSet::new();
    let mut morph_port_b = false;
    let run_result = (|| -> Result<()> {
        for mode in DISPLAY_MATRIX_MODES {
            if DISPLAY_MATRIX_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("display matrix interrupted".into());
            }
            if capture_usb_video && mode.id.starts_with("crt-") && !morph_port_b {
                morph_port_b = true;
                confirm_display_matrix_route("PORTB", "route Morph 4K to Port B")?;
            }
            let started = Instant::now();
            let session = connect(10)?;
            exec_checked(
                &session,
                "apply display matrix mode",
                &acknowledged_main_command(&format!(
                    "mister_magik_display_apply_headless_v1 mode={}",
                    mode.id
                )),
            )?;
            drop(session);
            let result = capture_display_matrix_mode(
                *mode,
                current_pid,
                &directory,
                &mut seen_hashes,
                &mut seen_usb_hashes,
                DisplayMatrixEvidence {
                    usb_video: capture_usb_video,
                    screensaver_wait_secs,
                },
                started,
            );
            if let Ok((_, new_pid)) = &result {
                current_pid = *new_pid;
            } else {
                let reconnect = connect(10);
                if let Ok(session) = reconnect {
                    let ready =
                        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(2));
                    if let Ok(ready) = ready {
                        current_pid = ready.launcher_pid;
                    }
                }
            }
            let session = connect(10)?;
            let rollback = exec_checked(
                &session,
                "rollback display matrix mode",
                &acknowledged_main_command("mister_magik_display_cancel_v1"),
            );
            drop(session);
            rollback?;
            let session = connect(10)?;
            let restored = wait_launcher_ready_after(
                &session,
                current_pid,
                Instant::now(),
                Duration::from_secs(15),
            )?;
            current_pid = restored.launcher_pid;
            drop(session);
            match result {
                Ok((entry, _)) => entries.push(entry),
                Err(error) => {
                    entries.push(json!({"mode": mode.id, "status": "fail", "error": error.to_string(), "elapsed_ms": started.elapsed().as_millis()}));
                    write_display_matrix_manifest(&directory, &original_mode, &entries)?;
                    return Err(error);
                }
            }
            write_display_matrix_manifest(&directory, &original_mode, &entries)?;
        }
        Ok(())
    })();
    let restore_result = restore_display_matrix_original(&original_mode, current_pid);
    let morph_restore_result = if morph_port_b {
        confirm_display_matrix_route("HDMI", "restore Morph 4K to HDMI")
    } else {
        Ok(())
    };
    unsafe {
        libc::signal(libc::SIGINT, previous_sigint);
    }
    write_display_matrix_manifest(&directory, &original_mode, &entries)?;
    run_result?;
    restore_result?;
    morph_restore_result?;
    println!("{}", directory.display());
    Ok(())
}

fn confirm_display_matrix_route(token: &str, instruction: &str) -> Result<()> {
    eprintln!("{instruction}, then type {token}:");
    let mut acknowledgement = String::new();
    io::stdin().read_line(&mut acknowledgement)?;
    if acknowledgement.trim() != token {
        return Err(format!("Morph 4K route was not confirmed with {token}").into());
    }
    Ok(())
}

fn wait_display_matrix_interval(duration: Duration) -> Result<()> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if DISPLAY_MATRIX_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("display matrix interrupted".into());
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
    Ok(())
}

fn parse_display_matrix_args(args: &[String]) -> Result<(&str, bool, Option<u64>)> {
    if args.first().map(String::as_str) != Some("--attended") {
        return Err("usage: mister display-matrix --attended --out DIRECTORY [--usb-video]".into());
    }
    let mut directory = None;
    let mut capture_usb_video = false;
    let mut screensaver_wait_secs = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                let value = args.get(index + 1).ok_or("--out needs DIRECTORY")?;
                if directory.replace(value.as_str()).is_some() {
                    return Err("--out may be specified only once".into());
                }
                index += 2;
            }
            "--usb-video" if !capture_usb_video => {
                capture_usb_video = true;
                index += 1;
            }
            "--screensaver-wait" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--screensaver-wait needs SECONDS")?
                    .parse::<u64>()?;
                if value == 0 || screensaver_wait_secs.replace(value).is_some() {
                    return Err("--screensaver-wait must be specified once with SECONDS > 0".into());
                }
                index += 2;
            }
            argument => {
                return Err(format!("unsupported display matrix argument: {argument}").into());
            }
        }
    }
    Ok((
        directory.ok_or("display matrix requires --out DIRECTORY")?,
        capture_usb_video,
        screensaver_wait_secs,
    ))
}

fn capture_display_matrix_mode(
    mode: DisplayMatrixMode,
    previous_pid: i64,
    directory: &Path,
    seen_hashes: &mut std::collections::HashSet<String>,
    seen_usb_hashes: &mut std::collections::HashSet<String>,
    evidence: DisplayMatrixEvidence,
    started: Instant,
) -> Result<(Value, i64)> {
    let session = connect(10)?;
    let ready = wait_launcher_ready_after(
        &session,
        previous_pid,
        Instant::now(),
        Duration::from_secs(15),
    )?;
    let readiness = exec_checked_output(
        &session,
        "display matrix readiness",
        &release_display_mode_command_for_runtime(),
    )?;
    drop(session);
    let readiness = parse_display_matrix_readiness(&readiness.stdout)?;
    let output = readiness.output;
    let framebuffer = readiness.framebuffer;
    let frames_before = readiness.frames_before;
    let frames_after = readiness.frames_after;
    validate_display_matrix_geometry(mode, output, framebuffer)?;
    if frames_after <= frames_before {
        return Err(format!("presentation did not advance for {}", mode.id).into());
    }
    let capture = request_framebuffer_png()?;
    validate_visible_launcher_capture(&capture)?;
    let path = directory.join(format!("{}.png", mode.id));
    fs::write(&path, &capture.png)?;
    let sha256 = encode_hex(&Sha256::digest(&capture.png));
    let width = capture
        .result
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let height = capture
        .result
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let stride = capture
        .result
        .get("stride")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bpp = capture
        .result
        .get("bpp")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let nonzero = capture
        .result
        .get("content_nonzero_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let varied = capture
        .result
        .get("content_varied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if width != framebuffer.0 as u64
        || height != framebuffer.1 as u64
        || png_dimensions(&capture.png)? != (width as u32, height as u32)
        || !valid_rgb565_stride(width, stride)
        || bpp != 16
        || nonzero == 0
        || !varied
    {
        return Err(format!("invalid or blank framebuffer capture for {}", mode.id).into());
    }
    if !seen_hashes.insert(sha256.clone()) {
        return Err(format!("stale or duplicate framebuffer capture for {}", mode.id).into());
    }
    let usb_video = if evidence.usb_video {
        let usb_path = directory.join(format!("{}-usb-video.jpg", mode.id));
        crt_qualification::capture_usb_video_frame(&usb_path)?;
        let bytes = fs::read(&usb_path)?;
        if bytes.len() < 1_024 {
            return Err(format!("USB Video capture is blank or truncated for {}", mode.id).into());
        }
        let usb_sha256 = encode_hex(&Sha256::digest(&bytes));
        if !seen_usb_hashes.insert(usb_sha256.clone()) {
            return Err(format!("stale or duplicate USB Video capture for {}", mode.id).into());
        }
        Some(json!({"path": usb_path, "bytes": bytes.len(), "sha256": usb_sha256}))
    } else {
        None
    };
    let screensaver = if let Some(wait_secs) = evidence.screensaver_wait_secs {
        wait_display_matrix_interval(Duration::from_secs(wait_secs))?;
        let saver_capture = request_framebuffer_png()?;
        validate_visible_launcher_capture(&saver_capture)?;
        let saver_sha256 = encode_hex(&Sha256::digest(&saver_capture.png));
        if saver_sha256 == sha256 || !seen_hashes.insert(saver_sha256.clone()) {
            return Err(format!(
                "screensaver did not advance to distinct content for {}",
                mode.id
            )
            .into());
        }
        let saver_path = directory.join(format!("{}-screensaver.png", mode.id));
        fs::write(&saver_path, &saver_capture.png)?;
        let saver_usb = if evidence.usb_video {
            let saver_usb_path = directory.join(format!("{}-screensaver-usb-video.jpg", mode.id));
            crt_qualification::capture_usb_video_frame(&saver_usb_path)?;
            let bytes = fs::read(&saver_usb_path)?;
            let saver_usb_sha256 = encode_hex(&Sha256::digest(&bytes));
            if bytes.len() < 1_024 || !seen_usb_hashes.insert(saver_usb_sha256.clone()) {
                return Err(
                    format!("invalid or stale screensaver USB Video for {}", mode.id).into(),
                );
            }
            Some(json!({"path": saver_usb_path, "bytes": bytes.len(), "sha256": saver_usb_sha256}))
        } else {
            None
        };
        Some(
            json!({"path": saver_path, "sha256": saver_sha256, "usb_video": saver_usb, "wait_secs": wait_secs}),
        )
    } else {
        None
    };
    Ok((
        json!({"mode": mode.id, "status": "pass", "path": path, "usb_video": usb_video, "screensaver": screensaver, "requested_output": mode.output.map(|(w,h)| format!("{w}x{h}")), "output_geometry": format!("{}x{}", output.0, output.1), "framebuffer_geometry": format!("{}x{}", framebuffer.0, framebuffer.1), "stride": stride, "capture_width": width, "capture_height": height, "bpp": bpp, "png_bytes": capture.png.len(), "sha256": sha256, "launcher_pid": ready.launcher_pid, "frames_before": frames_before, "frames_after": frames_after, "agent_elapsed_ms": capture.elapsed_ms, "elapsed_ms": started.elapsed().as_millis()}),
        ready.launcher_pid,
    ))
}

fn write_display_matrix_manifest(
    directory: &Path,
    original_mode: &str,
    entries: &[Value],
) -> Result<()> {
    let manifest = json!({"schema":"mister-magik-display-matrix-v2", "original_mode": original_mode, "captures": entries});
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(())
}

fn parse_display_reply_active(reply: &str) -> Result<String> {
    reply
        .split_whitespace()
        .find_map(|field| field.strip_prefix("active="))
        .map(str::to_owned)
        .ok_or_else(|| "display reply missing active mode".into())
}

fn parse_display_reply_pending(reply: &str) -> Result<Option<String>> {
    let pending = reply
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pending="))
        .ok_or("display reply missing pending mode")?;
    Ok((pending != "none").then(|| pending.to_owned()))
}

fn restore_display_matrix_original(original_mode: &str, previous_pid: i64) -> Result<()> {
    let session = connect(10)?;
    let state = exec_checked_output(
        &session,
        "query display transaction for cleanup",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if state
        .stdout
        .split_whitespace()
        .any(|field| field.starts_with("pending=") && field != "pending=none")
    {
        exec_checked(
            &session,
            "rollback display matrix cleanup",
            &acknowledged_main_command("mister_magik_display_cancel_v1"),
        )?;
        let _ = wait_launcher_ready_after(
            &session,
            previous_pid,
            Instant::now(),
            Duration::from_secs(15),
        )?;
    }
    let restored = exec_checked_output(
        &session,
        "verify original display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let active = parse_display_reply_active(restored.stdout.trim())?;
    if active != original_mode {
        return Err(format!("display matrix restored {active}, expected {original_mode}").into());
    }
    Ok(())
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32)> {
    if png.len() < 24 || &png[12..16] != b"IHDR" {
        return Err("PNG is missing IHDR dimensions".into());
    }
    Ok((
        u32::from_be_bytes(png[16..20].try_into()?),
        u32::from_be_bytes(png[20..24].try_into()?),
    ))
}

fn validate_display_matrix_geometry(
    mode: DisplayMatrixMode,
    output: (usize, usize),
    framebuffer: (usize, usize),
) -> Result<()> {
    if mode
        .output
        .is_some_and(|(w, h)| output != (usize::from(w), usize::from(h)))
    {
        return Err(format!(
            "unexpected output geometry for {}: {}x{}",
            mode.id, output.0, output.1
        )
        .into());
    }
    if mode
        .framebuffer
        .is_some_and(|expected| framebuffer != expected)
    {
        return Err(format!(
            "unexpected framebuffer geometry for {}: {}x{}",
            mode.id, framebuffer.0, framebuffer.1
        )
        .into());
    }
    Ok(())
}

fn parse_display_matrix_readiness(stdout: &str) -> Result<DisplayMatrixReadiness> {
    let plan = stdout
        .lines()
        .find_map(|line| line.strip_prefix("plan\t"))
        .ok_or("display readiness missing plan")?;
    let output = parse_geometry_token(plan, "output=")?;
    let framebuffer = parse_geometry_token(plan, "fb=")?;
    let frames = stdout
        .lines()
        .find_map(|line| line.strip_prefix("frames\t"))
        .ok_or("display readiness missing frame counters")?;
    let mut values = frames.split('\t');
    let before = values
        .next()
        .ok_or("missing initial frame counter")?
        .parse()?;
    let after = values
        .next()
        .ok_or("missing final frame counter")?
        .parse()?;
    let idle = stdout
        .lines()
        .find_map(|line| line.strip_prefix("idle\t"))
        .ok_or("display readiness missing idle state")?
        .parse()?;
    Ok(DisplayMatrixReadiness {
        output,
        framebuffer,
        frames_before: before,
        frames_after: after,
        idle,
    })
}

fn parse_geometry_token(text: &str, prefix: &str) -> Result<(usize, usize)> {
    let value = text
        .split_whitespace()
        .find_map(|field| field.strip_prefix(prefix))
        .ok_or_else(|| format!("display plan missing {prefix}"))?;
    let (width, height) = value.split_once('x').ok_or("invalid display geometry")?;
    Ok((width.parse()?, height.parse()?))
}

fn release_display_mode_command_for_runtime() -> String {
    "set -eu; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then root=/media/fat/mister-magik-dev; else root=/media/fat/mister-magik; fi; report=$(\"$root/mister-magik-fb\" latch-readiness-report --json); printf '%s\\n' \"$report\" | grep -Eq '\"state\"[[:space:]]*:[[:space:]]*\"ready\"'; plan=$(grep '^display-plan:' /tmp/mister-magik-slint.log | tail -n 1); before=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); idle=$(sed -n 's/.*\"idle\":[[:space:]]*\\(true\\|false\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$before\"; test -n \"$idle\"; after=$before; attempts=0; if test \"$idle\" != true; then while test \"$after\" -le \"$before\" && test \"$attempts\" -lt 10; do sleep 1; after=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$after\"; attempts=$((attempts+1)); done; test \"$after\" -gt \"$before\"; fi; printf 'plan\\t%s\\nframes\\t%s\\t%s\\nidle\\t%s\\nreadiness\\t%s\\n' \"$plan\" \"$before\" \"$after\" \"$idle\" \"$report\"".to_string()
}

fn delivery_health_command(layout: &str) -> Result<String> {
    let (main, directory) = match layout {
        "dev" => ("MiSTer_MagiKDev", "/media/fat/mister-magik-dev"),
        "public" => ("MiSTer_MagiK", "/media/fat/mister-magik"),
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok(format!(
        "set -eu; health_check=initializing; trap 'rc=$?; if test \"$rc\" -ne 0; then printf \"delivery_health_failure_tsv\\tcheck=%s\\trc=%s\\n\" \"$health_check\" \"$rc\" >&2; fi' EXIT; health_check=main-process; pidof {main} >/dev/null; health_check=launcher-process; pidof mister-magik-fb >/dev/null; health_check=scanout-module; grep -q '^mister_magik_scanout_slots ' /proc/modules; health_check=scanout-device; test -c /dev/mister-magik-scanout-slots; health_check=latch-readiness; report=$({directory}/mister-magik-fb latch-readiness-report); printf '%s\\n' \"$report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready'; health_check=launcher-env-clear; test ! -e {directory}/launcher.env; health_check=rebuild-clear; test ! -e {directory}/rebuild-on-next-boot; health_check=fault-launcher-env-clear; test ! -e /tmp/mister-magik/fs-fault-launcher.env; health_check=fault-session-clear; test ! -e /tmp/mister-magik/fs-fault-session; health_check=fault-json-clear; test ! -e /tmp/mister-magik/fs-fault.json; health_check=complete; trap - EXIT; printf 'delivery_health_tsv\\tvalid=1\\n'"
    ))
}

fn wait_delivery_health(session: &Session, layout: &str, timeout: Duration) -> Result<()> {
    let command = delivery_health_command(layout)?;
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        attempts = attempts.saturating_add(1);
        let output = exec(session, &command, true)?;
        if let Some(error) = exec_failure_message("delivery health", &output) {
            if started.elapsed() >= timeout {
                return Err(format!(
                    "{error}; delivery health attempts={attempts} elapsed_ms={}",
                    started.elapsed().as_millis()
                )
                .into());
            }
            thread::sleep(Duration::from_millis(250));
        } else {
            return Ok(());
        }
    }
}

fn validate_delivery_remote(remote: &str) -> Result<()> {
    if matches!(
        remote,
        "/media/fat/mister-magik/mister-magik-fb" | "/media/fat/mister-magik-dev/mister-magik-fb"
    ) {
        Ok(())
    } else {
        Err(format!("unsupported delivery remote: {remote}").into())
    }
}

fn validate_runtime_manifest_remote(remote: &str) -> Result<()> {
    if remote == "/media/fat/mister-magik-dev/platform-v3.manifest" {
        Ok(())
    } else {
        Err(format!("unsupported runtime manifest remote: {remote}").into())
    }
}

fn delivery_smoke_command(layout: &str, expected_sha256: &str) -> Result<String> {
    let (main, directory) = match layout {
        "dev" => ("MiSTer_MagiKDev", "/media/fat/mister-magik-dev"),
        "public" => ("MiSTer_MagiK", "/media/fat/mister-magik"),
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok(format!(
        "set -eu; smoke_check=initializing; status=/tmp/mister-magik/status.json; pid_before=; pid_after=; sequence_before=; sequence_after=; heartbeat_attempts=0; trap 'rc=$?; if test \"$rc\" -ne 0; then printf \"delivery_smoke_failure_tsv\\tcheck=%s\\trc=%s\\tpid_before=%s\\tpid_after=%s\\tsequence_before=%s\\tsequence_after=%s\\tattempts=%s\\n\" \"$smoke_check\" \"$rc\" \"$pid_before\" \"$pid_after\" \"$sequence_before\" \"$sequence_after\" \"$heartbeat_attempts\" >&2; fi' EXIT; smoke_check=artifact-sha256; test \"$(sha256sum {directory}/mister-magik-fb | awk '{{print $1}}')\" = '{expected_sha256}'; smoke_check=main-process; pidof {main} >/dev/null; smoke_check=launcher-process; pidof mister-magik-fb >/dev/null; {}; smoke_check=heartbeat-initial-pid; test -n \"$pid_before\"; smoke_check=heartbeat-initial-sequence; test -n \"$sequence_before\"; smoke_check=heartbeat-advance; while test \"$heartbeat_attempts\" -lt 10; do sleep 1; heartbeat_attempts=$((heartbeat_attempts+1)); candidate_pid=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); candidate_sequence=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); if test -z \"$candidate_pid\" || test -z \"$candidate_sequence\"; then continue; fi; pid_after=$candidate_pid; sequence_after=$candidate_sequence; if test \"$pid_after\" != \"$pid_before\"; then smoke_check=launcher-pid-stable; false; fi; if test \"$sequence_after\" -gt \"$sequence_before\"; then break; fi; done; smoke_check=heartbeat-final-pid; test -n \"$pid_after\"; smoke_check=heartbeat-final-sequence; test -n \"$sequence_after\"; smoke_check=heartbeat-advance; test \"$sequence_after\" -gt \"$sequence_before\"; smoke_check=launcher-scene; grep -Eq '\"scene\"[[:space:]]*:[[:space:]]*\"launcher\"' \"$status\"; smoke_check=effective-view; grep -Eq '\"effective_view\"[[:space:]]*:[[:space:]]*\"[^\"]+\"' \"$status\"; smoke_check=return-screen; grep -Eq '\"return_screen\"[[:space:]]*:[[:space:]]*\"[^\"]+\"' \"$status\"; smoke_check=rgb565; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; smoke_check=production-launcher-env-clear; test ! -e /media/fat/mister-magik/launcher.env; smoke_check=development-launcher-env-clear; test ! -e /media/fat/mister-magik-dev/launcher.env; smoke_check=production-rebuild-clear; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; smoke_check=development-rebuild-clear; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot; smoke_check=fault-launcher-env-clear; test ! -e /tmp/mister-magik/fs-fault-launcher.env; smoke_check=fault-session-clear; test ! -e /tmp/mister-magik/fs-fault-session; smoke_check=fault-json-clear; test ! -e /tmp/mister-magik/fs-fault.json; smoke_check=analytics-lease-clear; test ! -e /tmp/mister-magik/realtime-frame-analytics; smoke_check=screensaver-profile-clear; test ! -e /tmp/mister-magik/screensaver-profile; smoke_check=complete; trap - EXIT; printf 'delivery_smoke_tsv\\tvalid=1\\tpid=%s\\tsequence_before=%s\\tsequence_after=%s\\tattempts=%s\\n' \"$pid_after\" \"$sequence_before\" \"$sequence_after\" \"$heartbeat_attempts\"",
        launcher_heartbeat_initial_sample_command()
    ))
}

fn launcher_heartbeat_initial_sample_command() -> &'static str {
    "pid_before=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_before=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\")"
}

fn launcher_heartbeat_sample_command() -> &'static str {
    "status=/tmp/mister-magik/status.json; pid_before=; sequence_before=; pid_after=; sequence_after=; if test -r \"$status\"; then pid_before=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_before=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sleep 2; if test -r \"$status\"; then pid_after=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_after=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); fi; fi"
}

const RELEASE_TOKEN: &str = "/tmp/mister-magik/release-qualification-session";
const RELEASE_SNAPSHOT: &str = "/media/fat/mister-magik/release-qualification-snapshot";

fn release_rearm_token_command() -> String {
    format!(
        "mkdir -p /tmp/mister-magik; printf '%s\\n' attended-non-network-recovery-confirmed >{RELEASE_TOKEN}; test \"$(cat {RELEASE_TOKEN})\" = attended-non-network-recovery-confirmed"
    )
}

fn release_arming_cleanup_command() -> &'static str {
    "rm -f /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /tmp/mister-magik/latch-v4-qualification-control.tsv /tmp/mister-magik/latch-v4-qualification-control.tsv.tmp /tmp/mister-magik/latch-v4-qualification-state.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; rm -rf /tmp/mister-magik/latch-v4-catalog"
}

fn release_begin_command() -> String {
    let safety = platform_safety_script();
    let snapshot = format!(
        "snap={RELEASE_SNAPSHOT}; rm -rf \"$snap\"; mkdir -p \"$snap\"; if test -e /media/fat/MiSTer.ini; then cp -a /media/fat/MiSTer.ini \"$snap/MiSTer.ini\"; fi; printf '%s\\n' attended-non-network-recovery-confirmed >{RELEASE_TOKEN}; test -s {RELEASE_TOKEN}"
    );
    shell_sequence([
        "set -eu",
        release_arming_cleanup_command(),
        safety.as_str(),
        snapshot.as_str(),
    ])
}

fn release_catalog_command() -> String {
    format!(
        "set -eu; test -s {RELEASE_TOKEN}; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then root=/media/fat/mister-magik-dev; else root=/media/fat/mister-magik; fi; report=$(\"$root/mister-magik-fb\" catalog-v3-inspect); printf '%s\\n' \"$report\" | grep -Eq 'catalog_v3_summary_tsv[[:space:]]+valid=1'"
    )
}

fn release_handoff_command() -> String {
    format!(
        "set -eu; test -s {RELEASE_TOKEN}; grep -Eq '\"input_enabled\"[[:space:]]*:[[:space:]]*true' /tmp/mister-magik/status.json; test -p /dev/MiSTer_cmd; test ! -e /tmp/mister-magik/stale-launcher-return-state.json"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReleaseDisplayMode {
    label: &'static str,
    video_mode: &'static str,
    output: &'static str,
    framebuffer: &'static str,
    stride_bytes: usize,
}

const RELEASE_DISPLAY_MODES: [ReleaseDisplayMode; 6] = [
    ReleaseDisplayMode {
        label: "wide-768",
        video_mode: "10",
        output: "1366x768",
        framebuffer: "1366x768",
        stride_bytes: 2736,
    },
    ReleaseDisplayMode {
        label: "tall-1536",
        video_mode: "13",
        output: "2048x1536",
        framebuffer: "1024x768",
        stride_bytes: 2048,
    },
    ReleaseDisplayMode {
        label: "pixel-repeat-1440",
        video_mode: "14",
        output: "2560x1440",
        framebuffer: "1280x720",
        stride_bytes: 2560,
    },
    ReleaseDisplayMode {
        label: "hd-1080",
        video_mode: "8",
        output: "1920x1080",
        framebuffer: "960x540",
        stride_bytes: 1920,
    },
    ReleaseDisplayMode {
        label: "hd-720",
        video_mode: "0",
        output: "1280x720",
        framebuffer: "1280x720",
        stride_bytes: 2560,
    },
    ReleaseDisplayMode {
        label: "custom-1200",
        video_mode: "1920,1200,60",
        output: "1920x1200",
        framebuffer: "960x600",
        stride_bytes: 1920,
    },
];

fn release_display_mode_command(mode: ReleaseDisplayMode) -> String {
    format!(
        "set -eu; test -s {RELEASE_TOKEN}; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then root=/media/fat/mister-magik-dev; else root=/media/fat/mister-magik; fi; bin=\"$root/mister-magik-fb\"; test -x \"$bin\"; report=$(\"$bin\" latch-readiness-report --json); plan=$(grep '^display-plan:' /tmp/mister-magik-slint.log | tail -n 1 || true); latch=$(\"$bin\" fpga-latch-report); bpp=$(cat /sys/class/graphics/fb0/bits_per_pixel); printf 'release_display_readiness_json\\t%s\\n' \"$report\"; printf 'release_display_plan\\t%s\\n' \"$plan\"; printf 'release_display_latch\\t%s\\n' \"$latch\"; printf 'release_display_bpp\\t%s\\n' \"$bpp\"; printf '%s\\n' \"$report\" | grep -Eq '\"state\":\"ready\"'; printf '%s\\n' \"$report\" | grep -Eq '\"scanout_abi_version\":3'; printf '%s\\n' \"$report\" | grep -Eq '\"scanout_slot_capacity_bytes\":2101248'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_width\":1366'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_height\":768'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_stride_bytes\":2736'; printf '%s\\n' \"$plan\" | grep -Eq '^display-plan: .*output={output} .*fb={framebuffer} '; printf '%s\\n' \"$latch\" | grep -q 'supported=1'; printf '%s\\n' \"$latch\" | grep -q 'drop_count=0'; test \"$bpp\" = 16; printf 'display_qualification_tsv\\tlabel={label}\\tvideo_mode={video_mode}\\toutput={output}\\tfb={framebuffer}\\tstride={stride}\\n'",
        label = mode.label,
        video_mode = mode.video_mode,
        output = mode.output,
        framebuffer = mode.framebuffer,
        stride = mode.stride_bytes,
    )
}

fn qualify_release_display_matrix_with(
    connection: &ConnectionConfig,
    agent: &AgentEndpoint,
) -> Result<String> {
    for mode in RELEASE_DISPLAY_MODES {
        let session = connect_with(connection, 10)?;
        exec_checked(
            &session,
            "release display token",
            &format!("test -s {RELEASE_TOKEN}"),
        )?;
        edit_remote_ini(
            &session,
            IniEdit::MenuMode(mode.video_mode.to_string()),
            false,
        )?;
        exec_checked(&session, "release display sync", "sync")?;
        issue_reboot(&session, RebootMode::Supervised)?;
        drop(session);
        if !wait_down_with(connection, 40.0) || wait_up_with(connection, 120.0)? != 0 {
            return Err(format!("{} did not complete its reboot transition", mode.label).into());
        }
        let session = connect_with(connection, 10)?;
        exec_checked(
            &session,
            "release display rearm token",
            &release_rearm_token_command(),
        )?;
        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
        exec_checked(
            &session,
            &format!("release display {}", mode.label),
            &release_display_mode_command(mode),
        )?;
        drop(session);
        capture_buffer_at(agent, &[])?;
    }
    Ok(format!(
        "display=qualified modes={} captures={}",
        RELEASE_DISPLAY_MODES.len(),
        RELEASE_DISPLAY_MODES.len()
    ))
}

fn release_recovery_command() -> String {
    let preflight = format!(
        "test \"$(cat {RELEASE_TOKEN})\" = attended-non-network-recovery-confirmed; test -p /dev/MiSTer_cmd"
    );
    let safety = platform_safety_script();
    shell_sequence([
        "set -eu",
        preflight.as_str(),
        release_arming_cleanup_command(),
        safety.as_str(),
    ])
}

fn release_restore_command() -> String {
    let snapshot = format!(
        "snap={RELEASE_SNAPSHOT}; {}; if test -s \"$snap/MiSTer.ini\"; then cp -a \"$snap/MiSTer.ini\" /media/fat/MiSTer.ini; fi; rm -f {RELEASE_TOKEN}; rm -rf \"$snap\"",
        release_arming_cleanup_command()
    );
    let safety = platform_safety_script();
    let verify = format!("test ! -e {RELEASE_TOKEN}");
    shell_sequence([
        "set -eu",
        snapshot.as_str(),
        safety.as_str(),
        verify.as_str(),
    ])
}

fn diagnostic_facts_command() -> String {
    format!(
        "set -eu; main=false; launcher=false; agent=false; credentials=false; scanout=false; firmware=false; latch=false; unstable=false; temporary=false; launcher_heartbeat_advancing=false; {{ pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }} && main=true; pidof mister-magik-fb >/dev/null 2>&1 && launcher=true; pidof mister-magik-agent >/dev/null 2>&1 && agent=true; test -s /media/fat/mister-magik-dev/agent.token && credentials=true; {{ grep -q '^mister_magik_scanout_slots ' /proc/modules 2>/dev/null && test -c /dev/mister-magik-scanout-slots; }} && scanout=true; \"$scanout\" && firmware=true; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then root=/media/fat/mister-magik-dev; else root=/media/fat/mister-magik; fi; if test -x \"$root/mister-magik-fb\"; then latch_report=$(\"$root/mister-magik-fb\" latch-readiness-report 2>/dev/null || true); printf '%s\\n' \"$latch_report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready' && latch=true; fi; {}; if test -n \"$pid_before\" && test \"$pid_before\" = \"$pid_after\" && test -n \"$sequence_before\" && test -n \"$sequence_after\" && test \"$sequence_after\" -gt \"$sequence_before\"; then launcher_heartbeat_advancing=true; fi; test -e /tmp/mister-magik/reboot-unstable && unstable=true; arming=0; for path in /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; do test ! -e \"$path\" || arming=$((arming + 1)); done; for path in /tmp/mister-magik/agent-benchmark.tsv /tmp/mister-magik/agent-benchmark-warmup.tsv /tmp/mister-magik/agent-cold-benchmark.out /tmp/mister-magik/stale-launcher-return-state.json /tmp/mister-magik/realtime-frame-analytics /tmp/mister-magik/screensaver-profile; do test ! -e \"$path\" || temporary=true; done; printf '{{\"main_running\":%s,\"launcher_running\":%s,\"agent_running\":%s,\"credentials_ready\":%s,\"firmware_compatible\":%s,\"scanout_ready\":%s,\"latch_ready\":%s,\"reboot_unstable\":%s,\"arming_files\":%s,\"temporary_state\":%s,\"launcher_heartbeat_advancing\":%s}}\\n' \"$main\" \"$launcher\" \"$agent\" \"$credentials\" \"$firmware\" \"$scanout\" \"$latch\" \"$unstable\" \"$arming\" \"$temporary\" \"$launcher_heartbeat_advancing\"",
        launcher_heartbeat_sample_command()
    )
}

fn clear_latch_diagnostics_command() -> &'static str {
    "set -eu; rm -rf /media/fat/mister-magik/diagnostics/latch /media/fat/mister-magik-dev/diagnostics/latch; mkdir -p /media/fat/mister-magik/diagnostics/latch /media/fat/mister-magik-dev/diagnostics/latch; test -z \"$(find /media/fat/mister-magik/diagnostics/latch /media/fat/mister-magik-dev/diagnostics/latch -mindepth 1 -print -quit)\"; printf 'latch_diagnostics_clear_tsv\\tvalid=1\\tpublic=empty\\tdevelopment=empty\\n'"
}

fn is_safe_crash_report_path(path: &str) -> bool {
    [
        "/media/fat/mister-magik/crashes/report-",
        "/media/fat/mister-magik-dev/crashes/report-",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        && path.ends_with(".json")
        && !path.contains("..")
}

fn safe_repair_command() -> String {
    let safety = platform_safety_script();
    shell_sequence([
        "set -eu",
        "rm -f /tmp/mister-magik/agent-benchmark.tsv /tmp/mister-magik/agent-benchmark-warmup.tsv /tmp/mister-magik/agent-cold-benchmark.out /tmp/mister-magik/stale-launcher-return-state.json /tmp/mister-magik/realtime-frame-analytics",
        "rm -rf /tmp/mister-magik/screensaver-profile",
        release_arming_cleanup_command(),
        safety.as_str(),
    ])
}

fn arming_status() -> Result<()> {
    let session = connect(10)?;
    let command = "set -eu; found=0; for path in /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; do if test -e \"$path\"; then printf 'armed=%s\\n' \"$path\"; found=1; fi; done; test \"$found\" = 1 || echo arming=clear";
    let output = exec(&session, command, false)?;
    if let Some(message) = exec_failure_message("arming status", &output) {
        return Err(message.into());
    }
    print!("{}", output.stdout);
    Ok(())
}

fn core_list() -> Result<()> {
    let session = connect(10)?;
    let command = "set -eu; for directory in _Console _Computer _Arcade/cores _LLAPI; do find \"/media/fat/$directory\" -maxdepth 3 -type f -name '*.rbf' -printf '%s\\t%T@\\t%p\\n' 2>/dev/null || true; done";
    let output = exec(&session, command, false)?;
    if let Some(message) = exec_failure_message("core list", &output) {
        return Err(message.into());
    }
    print!("{}", output.stdout);
    Ok(())
}

fn mode_cli(args: &[String]) -> Result<()> {
    let mode = args.first().map(String::as_str).unwrap_or("status");
    if args.len() > 1 || !matches!(mode, "status" | "dev" | "public" | "stock") {
        return Err("usage: mister mode <status|dev|public|stock>".into());
    }
    let session = connect(10)?;
    if mode == "status" {
        let status = collect_status(&session)?;
        print_status_summary(&status);
        arming_status()?;
        return Ok(());
    }
    let selection = match mode {
        "dev" => {
            exec_checked(
                &session,
                "development platform verify",
                &installed_platform_verify_command(Layout::Development),
            )?;
            "MiSTer_MagiKDev"
        }
        "public" => {
            exec_checked(
                &session,
                "public platform verify",
                &installed_platform_verify_command(Layout::Public),
            )?;
            "MiSTer_MagiK"
        }
        "stock" => "MiSTer",
        _ => unreachable!(),
    };
    ensure_stock_inittab(&session, false)?;
    edit_remote_ini(&session, IniEdit::SelectMain(selection.into()), false)?;
    let safety = platform_safety_script();
    let arming_cleanup =
        shell_sequence(["set -eu", release_arming_cleanup_command(), safety.as_str()]);
    exec_checked(&session, "mode arming cleanup", &arming_cleanup)?;
    issue_reboot(&session, RebootMode::Supervised)?;
    drop(session);
    if !wait_down(40.0) || wait_up(120.0)? != 0 {
        return Err("mode switch did not complete its bounded reboot transition".into());
    }
    Ok(())
}

fn scene_cli(args: &[String]) -> Result<()> {
    let scene = args.first().map(String::as_str).unwrap_or("launcher");
    if !matches!(
        scene,
        "launcher" | "controller_test" | "tear_pattern" | "video_playback" | "crt_trial"
    ) {
        return Err(
            "usage: mister scene <launcher|controller_test|tear_pattern|video_playback|crt_trial> [seconds]"
                .into(),
        );
    }
    let seconds = args
        .get(1)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(if scene == "launcher" {
            0
        } else if scene == "crt_trial" {
            30
        } else {
            10
        });
    if scene == "crt_trial" && seconds != 30 {
        return Err("crt_trial duration is fixed at 30 seconds".into());
    }
    if args.len() > 2 || seconds > 3_600 || (scene != "launcher" && seconds == 0) {
        return Err("scene duration must be 1..=3600 seconds".into());
    }
    let session = connect(10)?;
    if scene == "launcher" {
        return launcher_restart(
            &session,
            &LauncherRestartOptions {
                clear_env: true,
                ..LauncherRestartOptions::default()
            },
        );
    }
    let runtime_settings = if scene == "crt_trial" {
        let output = exec_checked_output(
            &session,
            "resolved CRT mode",
            &acknowledged_main_command("mister_magik_settings_get_v1"),
        )?;
        Some(parse_crt_runtime_settings_reply(&output.stdout)?)
    } else {
        None
    };
    exec_checked(
        &session,
        "scene suspend",
        &acknowledged_main_command("mister_magik_suspend"),
    )?;
    let run_command = if let Some(runtime_settings) = runtime_settings.as_deref() {
        crt_trial_run_command(runtime_settings, None)
    } else {
        format!(
            "set -eu; test -x /media/fat/mister-magik-dev/mister-magik-fb; /media/fat/mister-magik-dev/mister-magik-fb ui {scene} {seconds} >/tmp/mister-magik-{scene}.log 2>&1"
        )
    };
    let run = exec_checked(&session, "operator scene", &run_command);
    if scene == "crt_trial" {
        run?;
        let output = exec_checked_output(
            &session,
            "CRT trial status",
            "tail -n 128 /tmp/mister-magik-crt_trial.log",
        )?;
        println!("{}", parse_crt_trial_status(&output.stdout)?);
        Ok(())
    } else {
        let resume = exec_checked(
            &session,
            "scene resume",
            &acknowledged_main_command("mister_magik_resume"),
        );
        run.and(resume)
    }
}

fn parse_crt_runtime_settings_reply(output: &str) -> Result<String> {
    let settings = output
        .trim()
        .strip_prefix("ok SettingsV1 ")
        .ok_or("Main did not return runtime settings v1")?;
    let mode = settings
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("output="))
        .ok_or("Main runtime settings omitted output")?;
    if !matches!(
        mode,
        "crt-240p60" | "crt-288p50" | "crt-480p60" | "crt-576p50"
    ) {
        return Err(format!("CRT trial requires a resolved standard CRT mode, got {mode}").into());
    }
    Ok(format!("schema=1&output={mode}"))
}

fn crt_trial_run_command(runtime_settings: &str, rectangle: Option<[u16; 4]>) -> String {
    let resume = acknowledged_main_command("mister_magik_resume");
    let diagnostic = rectangle.map_or_else(String::new, |[left, right, top, bottom]| {
        if runtime_settings.contains("output=crt-576p50") {
            format!(
                "MISTER_MAGIK_CRT_TRIAL=1 MISTER_FB_DIAGNOSTIC_RECT=45,684,{top},{bottom} MISTER_CRT_TRIAL_CONTENT_BOUNDS={left},{right} "
            )
        } else {
            format!(
                "MISTER_MAGIK_CRT_TRIAL=1 MISTER_FB_DIAGNOSTIC_RECT={left},{right},{top},{bottom} "
            )
        }
    });
    format!(
        "cleanup() {{ trap - EXIT HUP INT TERM; {resume}; }}; trap cleanup EXIT HUP INT TERM; set -eu; test -x /media/fat/mister-magik-dev/mister-magik-fb; {diagnostic}MISTER_MAGIK_RUNTIME_SETTINGS_V1={} /media/fat/mister-magik-dev/mister-magik-fb ui crt_trial 30 >/tmp/mister-magik-crt_trial.log 2>&1",
        sh(runtime_settings),
    )
}

fn run_crt_geometry_trial_with(
    connection: &ConnectionConfig,
    rectangle: [u16; 4],
) -> Result<String> {
    // The remote trial trap resumes Main after success, failure, or disconnect.
    let settings_session = connect_with(connection, 10)?;
    let output = exec_checked_output(
        &settings_session,
        "resolved CRT mode",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let runtime_settings = parse_crt_runtime_settings_reply(&output.stdout)?;
    validate_crt_geometry_trial(&runtime_settings, rectangle)?;
    drop(settings_session);

    let output = crt_geometry_capture_path(
        &std::env::temp_dir(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    );
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let runtime_settings_for_trial = runtime_settings.clone();
    let trial_connection = connection.clone();
    let trial = std::thread::spawn(move || -> std::result::Result<(), String> {
        let session = connect_with(&trial_connection, 10).map_err(|error| error.to_string())?;
        exec_checked(
            &session,
            "geometry trial suspend",
            &acknowledged_main_command("mister_magik_suspend"),
        )
        .map_err(|error| error.to_string())?;
        if ready_tx.send(()).is_err() {
            exec_checked(
                &session,
                "geometry trial resume after observer disconnect",
                &acknowledged_main_command("mister_magik_resume"),
            )
            .map_err(|error| error.to_string())?;
            return Err("geometry trial observer disconnected".to_owned());
        }
        let result = exec_checked(
            &session,
            "geometry trial",
            &crt_trial_run_command(&runtime_settings_for_trial, Some(rectangle)),
        );
        if let Err(error) = result {
            let recovery = connect_with(&trial_connection, 10).and_then(|recovery| {
                exec_checked(
                    &recovery,
                    "geometry trial compensating resume",
                    &acknowledged_main_command("mister_magik_resume"),
                )
            });
            return match recovery {
                Ok(()) => Err(error.to_string()),
                Err(recovery_error) => Err(format!(
                    "{error}; compensating Main resume failed: {recovery_error}"
                )),
            };
        }
        Ok(())
    });
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "geometry trial did not start")?;
    std::thread::sleep(Duration::from_secs(2));
    let capture = crt_qualification::capture_usb_video_frame(&output);
    let trial_result = trial.join().map_err(|_| "geometry trial worker panicked")?;
    trial_result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    capture?;
    let recovery_session = connect_with(connection, 10)?;
    let ready = wait_launcher_ready(&recovery_session, Instant::now(), Duration::from_secs(15))?;
    let trial_status = exec_checked_output(
        &recovery_session,
        "geometry trial status",
        "tail -n 128 /tmp/mister-magik-crt_trial.log",
    )?;
    let trial_status = parse_crt_trial_status(&trial_status.stdout)?;
    let content_bounds = runtime_settings
        .contains("output=crt-576p50")
        .then_some([rectangle[0], rectangle[1]]);
    let destination_rectangle = if content_bounds.is_some() {
        [45, 684, rectangle[2], rectangle[3]]
    } else {
        rectangle
    };
    Ok(json!({
        "runtime_settings": runtime_settings,
        "destination_rectangle": destination_rectangle,
        "content_bounds": content_bounds,
        "usb_video": output,
        "trial_status": trial_status,
        "launcher_pid": ready.launcher_pid,
    })
    .to_string())
}

fn crt_geometry_capture_path(temporary_directory: &Path, timestamp_ms: u128) -> PathBuf {
    temporary_directory.join(format!("mister-magik-crt-geometry-{timestamp_ms}.jpg"))
}

fn crt_screensaver_trial_run_command(runtime_settings: &str, duration_secs: u64) -> String {
    let resume = acknowledged_main_command("mister_magik_resume");
    format!(
        "cleanup() {{ trap - EXIT HUP INT TERM; rm -f /tmp/mister-magik/realtime-frame-analytics; {resume}; }}; trap cleanup EXIT HUP INT TERM; set -eu; test -x /media/fat/mister-magik-dev/mister-magik-fb; mkdir -p /tmp/mister-magik; printf 'wall\\n' >/tmp/mister-magik/realtime-frame-analytics; run_rc=0; MISTER_MAGIK_RUNTIME_SETTINGS_V1={} MISTER_SCREENSAVER_START_ACTIVE=1 /media/fat/mister-magik-dev/mister-magik-fb ui launcher {duration_secs} >/tmp/mister-magik-crt-screensaver.log 2>&1 || run_rc=$?; cp /tmp/mister-magik/status.json /tmp/mister-magik/crt-screensaver-status.json; test \"$run_rc\" -eq 0",
        sh(runtime_settings),
    )
}

fn run_crt_screensaver_trial_with(
    connection: &ConnectionConfig,
    duration_secs: u64,
    capture_delay_secs: u64,
) -> Result<String> {
    if duration_secs == 0 || capture_delay_secs == 0 || capture_delay_secs >= duration_secs {
        return Err("screensaver trial timing must satisfy 0 < capture delay < duration".into());
    }
    // The remote trial trap removes its analytics lease and resumes Main after
    // success, failure, or disconnect.
    let settings_session = connect_with(connection, 10)?;
    let output = exec_checked_output(
        &settings_session,
        "resolved CRT mode",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let runtime_settings = parse_crt_runtime_settings_reply(&output.stdout)?;
    drop(settings_session);

    let usb_video = std::env::temp_dir().join(format!(
        "mister-magik-crt-screensaver-{}.jpg",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let runtime_settings_for_trial = runtime_settings.clone();
    let trial_connection = connection.clone();
    let trial = std::thread::spawn(move || -> std::result::Result<(), String> {
        let session = connect_with(&trial_connection, 10).map_err(|error| error.to_string())?;
        exec_checked(
            &session,
            "screensaver trial suspend",
            &acknowledged_main_command("mister_magik_suspend"),
        )
        .map_err(|error| error.to_string())?;
        if ready_tx.send(()).is_err() {
            exec_checked(
                &session,
                "screensaver trial resume after observer disconnect",
                &acknowledged_main_command("mister_magik_resume"),
            )
            .map_err(|error| error.to_string())?;
            return Err("screensaver trial observer disconnected".to_owned());
        }
        let result = exec_checked(
            &session,
            "screensaver trial",
            &crt_screensaver_trial_run_command(&runtime_settings_for_trial, duration_secs),
        );
        if let Err(error) = result {
            let recovery = connect_with(&trial_connection, 10).and_then(|recovery| {
                exec_checked(
                    &recovery,
                    "screensaver trial compensating resume",
                    &acknowledged_main_command("mister_magik_resume"),
                )
            });
            return match recovery {
                Ok(()) => Err(error.to_string()),
                Err(recovery_error) => Err(format!(
                    "{error}; compensating Main resume failed: {recovery_error}"
                )),
            };
        }
        Ok(())
    });
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "screensaver trial did not start")?;
    std::thread::sleep(Duration::from_secs(capture_delay_secs));
    let capture = crt_qualification::capture_usb_video_frame(&usb_video);
    let trial_result = trial
        .join()
        .map_err(|_| "screensaver trial worker panicked")?;
    trial_result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    capture?;

    let recovery_session = connect_with(connection, 10)?;
    let ready = wait_launcher_ready(&recovery_session, Instant::now(), Duration::from_secs(15))?;
    let status = exec_checked_output(
        &recovery_session,
        "screensaver trial status",
        "cat /tmp/mister-magik/crt-screensaver-status.json",
    )?;
    let status: Value = serde_json::from_str(status.stdout.trim())?;
    let log = exec_checked_output(
        &recovery_session,
        "screensaver trial log",
        "grep -E 'screensaver_startup_timing|screensaver_loader|screensaver_scaler|launcher fps|latch_failure_tsv' /tmp/mister-magik-crt-screensaver.log | tail -n 80",
    )?;
    let rendered_frames = status.get("frames").and_then(Value::as_u64).unwrap_or(0);
    if rendered_frames == 0 || !log.stdout.contains("screensaver_loader path=") {
        return Err("product screensaver did not reach its first presented frame".into());
    }
    let bytes = fs::read(&usb_video)?;
    let usb_sha256 = encode_hex(&Sha256::digest(&bytes));
    Ok(json!({
        "runtime_settings": runtime_settings,
        "status": status,
        "log_tail": log.stdout.trim(),
        "usb_video": usb_video,
        "usb_bytes": bytes.len(),
        "usb_sha256": usb_sha256,
        "launcher_pid": ready.launcher_pid,
    })
    .to_string())
}

fn run_crt_screensaver_matrix_with(connection: &ConnectionConfig) -> Result<String> {
    let session = connect_with(connection, 10)?;
    let original_reply = exec_checked_output(
        &session,
        "query original display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("CRT screensaver matrix cannot start during a display transaction".into());
    }
    let mut current_pid =
        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?.launcher_pid;
    drop(session);

    let mut entries = Vec::new();
    let run_result = (|| -> Result<()> {
        for mode in crt_screensaver_matrix_modes() {
            let session = connect_with(connection, 10)?;
            exec_checked(
                &session,
                "apply CRT screensaver matrix mode",
                &acknowledged_main_command(&format!(
                    "mister_magik_display_apply_headless_v1 mode={}",
                    mode.id
                )),
            )?;
            drop(session);
            let session = connect_with(connection, 10)?;
            current_pid = wait_launcher_ready_after(
                &session,
                current_pid,
                Instant::now(),
                Duration::from_secs(15),
            )?
            .launcher_pid;
            drop(session);

            let detail: Value =
                serde_json::from_str(&run_crt_screensaver_trial_with(connection, 8, 4)?)?;
            current_pid = detail
                .get("launcher_pid")
                .and_then(Value::as_i64)
                .ok_or("screensaver matrix trial omitted restored launcher pid")?;
            entries.push(json!({"mode": mode.id, "result": detail}));

            let session = connect_with(connection, 10)?;
            exec_checked(
                &session,
                "rollback CRT screensaver matrix mode",
                &acknowledged_main_command("mister_magik_display_cancel_v1"),
            )?;
            let restored = wait_launcher_ready_after(
                &session,
                current_pid,
                Instant::now(),
                Duration::from_secs(15),
            )?;
            current_pid = restored.launcher_pid;
        }
        Ok(())
    })();
    let restore_result = restore_display_matrix_original(&original_mode, current_pid);
    run_result?;
    restore_result?;
    Ok(json!({
        "schema": "mister-magik-crt-screensaver-matrix-v1",
        "original_mode": original_mode,
        "modes": entries,
    })
    .to_string())
}

fn crt_screensaver_matrix_modes() -> impl Iterator<Item = &'static DisplayMatrixMode> {
    DISPLAY_MATRIX_MODES
        .iter()
        .filter(|mode| mode.id.starts_with("crt-"))
}

fn validate_crt_geometry_trial(runtime_settings: &str, rectangle: [u16; 4]) -> Result<()> {
    let (baseline, variable_axis, safety_window) = if runtime_settings.contains("output=crt-288p50")
    {
        ([67, 706, 12, 299], "vertical", 64)
    } else if runtime_settings.contains("output=crt-576p50") {
        ([45, 684, 40, 615], "horizontal", 192)
    } else {
        return Err("geometry trials are limited to crt-288p50 and crt-576p50".into());
    };
    let [left, right, top, bottom] = rectangle;
    if left > right || top > bottom {
        return Err("geometry trial rectangle is unordered".into());
    }
    let delta = |value: u16, expected: u16| value.abs_diff(expected) <= safety_window;
    if !rectangle
        .iter()
        .zip(baseline)
        .all(|(value, expected)| delta(*value, expected))
    {
        return Err("geometry trial rectangle exceeds the mode safety window".into());
    }
    let fixed_axis_matches = match variable_axis {
        "vertical" => left == baseline[0] && right == baseline[1],
        "horizontal" => top == baseline[2] && bottom <= baseline[3] && bottom >= baseline[3] - 8,
        _ => false,
    };
    if !fixed_axis_matches {
        return Err("288p trials are vertical-only and 576p trials are horizontal-only".into());
    }
    Ok(())
}

fn parse_crt_trial_status(output: &str) -> Result<&str> {
    const MARKERS: [&str; 3] = [
        "crt_trial_status_v2 schema=2 ",
        "crt_trial_status_v3 schema=3 ",
        "crt_trial_status_v4 schema=4 ",
    ];
    let status = output
        .match_indices("crt_trial_status_v")
        .map(|(offset, _)| offset)
        .last()
        .map(|offset| &output[offset..])
        .unwrap_or(output)
        .lines()
        .next()
        .unwrap_or_default()
        .trim();
    let marker = MARKERS
        .iter()
        .find(|marker| status.starts_with(**marker))
        .copied();
    if marker.is_none() {
        return Err(format!(
            "CRT trial did not return a typed status response: {}",
            status.replace(['\t', '\n', '\r'], " ")
        )
        .into());
    }
    if status.split_ascii_whitespace().any(|field| field == "ok=0") {
        return Err(format!("CRT trial reported failure: {status}").into());
    }
    for required in [
        "ok=1",
        "mode=crt-",
        "duration_ms=",
        "frames=",
        "flips=",
        "reason=none",
    ] {
        if !status
            .split_ascii_whitespace()
            .any(|field| field.starts_with(required))
        {
            return Err(format!("CRT trial status omitted successful {required}").into());
        }
    }
    if marker != Some(MARKERS[0]) {
        for required in [
            "posts=",
            "drops=",
            "final_pending=",
            "final_active_matches=",
            "unsafe_active_writes=",
            "pending_writes=",
            "alternation_misses=",
            "cadence_misses=",
            "max_interval_us=",
            "max_settle_us=",
            "max_render_us=",
            "max_copy_us=",
            "max_status_us=",
            "post_status_retry_frames=",
            "max_post_status_reads=",
            "last_buffer=",
            "last_sequence=",
        ] {
            if !status
                .split_ascii_whitespace()
                .any(|field| field.starts_with(required))
            {
                return Err(format!("CRT trial status omitted diagnostic {required}").into());
            }
        }
    }
    if marker == Some(MARKERS[2]) {
        for required in [
            "post_status_transport_retry_frames=",
            "max_post_status_wire_attempts=",
        ] {
            if !status
                .split_ascii_whitespace()
                .any(|field| field.starts_with(required))
            {
                return Err(format!("CRT trial status omitted diagnostic {required}").into());
            }
        }
    }
    Ok(status)
}

const SCREENSAVER_PROFILE_REMOTE_DIR: &str = "/tmp/mister-magik/screensaver-profile";
const CATALOG_LIFECYCLE_REMOTE_DIR: &str = "/tmp/mister-magik/catalog-lifecycle-benchmark";
const CATALOG_LIFECYCLE_FIRST_VISIBLE_TIMEOUT_SECS: u64 = 60;
const CATALOG_LIFECYCLE_COMPLETE_TIMEOUT_SECS: u64 = 20 * 60;
const ALPHA_CATALOG_COMPLETE_TIMEOUT_SECS: u64 = 8 * 60;
const SCREENSAVER_STARTUP_WARMUP_FRAMES: usize = 3;
const SCREENSAVER_PROFILE_DURATION_SECS: u64 = 45;
const SCREENSAVER_PROFILE_TIMEOUT_SECS: u64 = SCREENSAVER_PROFILE_DURATION_SECS + 20;
const SCREENSAVER_POPULATED_WINDOW_SECS: u64 = 15;
const PARTICLE_SEARCH_TRIAL_SECS: u64 = 12;
const PARTICLE_CONFIRMATION_SECS: u64 = 30;
const PARTICLE_DEMO_40K_DURATION_SECS: u64 = 15;
const PARTICLE_DEMO_40K_COUNT: u64 = 40_960;
const PARTICLE_STEP_DURATION_SECS: u64 = 20;
const PARTICLE_STEP_COUNT: u64 = 14_336;
const PARTICLE_CPU_PROFILE_DURATION_SECS: u64 = 30;
const PARTICLE_CPU_PROFILE_CAPACITY_COUNT: u64 = 14_336;
const PARTICLE_CPU_PROFILE_VISUAL_COUNT: u64 = 9_216;
const PARTICLE_SHOWCASE_DURATION_SECS: u64 = 30;
const PARTICLE_COUNT_STEP: u64 = 1_024;
const PARTICLE_COUNT_MAX: u64 = 524_288;
const PARTICLE_POST_RESERVE_US: u64 = 750;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParticleBenchmarkRun {
    Complete,
    Capacity,
    Demo40k,
    Step,
    Showcase(u8),
}

fn particle_showcase_demo(number: u8) -> Result<(&'static str, u64)> {
    match number {
        1 => Ok(("solar-chrysanthemum", 1_728)),
        2 => Ok(("recursive-halo", 1_732)),
        3 => Ok(("copper-willow-rain", 1_624)),
        4 => Ok(("phoenix-comet", 2_272)),
        5 => Ok(("magnetic-flower", 1_958)),
        6 => Ok(("oled-peony", 1_644)),
        7 => Ok(("solar-chrysanthemum-v2", 2_406)),
        8 => Ok(("recursive-halo-v2", 2_274)),
        9 => Ok(("copper-willow-rain-v2", 2_148)),
        10 => Ok(("phoenix-comet-v2", 6_640)),
        11 => Ok(("magnetic-flower-v2", 2_128)),
        12 => Ok(("oled-peony-v2", 1_228)),
        13 => Ok(("fire-embers", 20_480)),
        14 => Ok(("spiral-galaxy", 81_920)),
        15 => Ok(("warp-speed", 45_056)),
        16 => Ok(("meteor-shower", 20_480)),
        17 => Ok(("weather", 49_152)),
        18 => Ok(("particle-portal", 65_536)),
        19 => Ok(("electric-storm", 16_384)),
        20 => Ok(("fountain-waterfall", 32_768)),
        21 => Ok(("arcade-cabinet", 12_288)),
        22 => Ok(("procedural-sprite-materials", 16_384)),
        23 => Ok(("variable-width-ribbons", 8_192)),
        24 => Ok(("curl-noise-flow-field", 32_768)),
        25 => Ok(("density-bloom", 24_576)),
        26 => Ok(("layered-child-systems", 4_096)),
        27 => Ok(("spatial-field-stack", 24_576)),
        28 => Ok(("depth-aware-material-lod", 40_960)),
        29 => Ok(("source-morph", 12_288)),
        30 => Ok(("sdf-collision", 8_192)),
        31 => Ok(("grid-flocking", 12_288)),
        32 => Ok(("fractal-grid-terrain", 49_152)),
        33 => Ok(("layer-mapped-hologram", 40_960)),
        34 => Ok(("spherical-field-observatory", 32_768)),
        35 => Ok(("twisted-multi-form-cathedral", 65_536)),
        36 => Ok(("point-cloud-morph-passage", 24_576)),
        _ => Err(format!("particle showcase demo must be in 1..=36, received {number}").into()),
    }
}

fn last_json_line(output: &str) -> Option<Value> {
    output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
}

fn profile_installed_search(config: &NativeDeviceConfig, output_dir: &Path) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    fs::create_dir_all(output_dir)?;
    let started = Instant::now();
    let mut log = String::new();
    let output = loop {
        let output = exec(
            &session,
            "/media/fat/mister-magik-dev/mister-magik-fb search-bench",
            true,
        )?;
        log.push_str(&output.stdout);
        log.push_str(&output.stderr);
        if exec_failure_message("installed search benchmark", &output).is_none() {
            break output;
        }
        if !search_benchmark_waits_for_catalog(&output)
            || started.elapsed() >= Duration::from_secs(180)
        {
            fs::write(output_dir.join("search-bench.log"), &log)?;
            return Err(exec_failure_message("installed search benchmark", &output)
                .expect("failed search benchmark checked above")
                .into());
        }
        thread::sleep(Duration::from_secs(1));
    };
    fs::write(output_dir.join("search-bench.log"), &log)?;
    let summary =
        last_json_line(&output.stdout).ok_or("installed search benchmark returned no JSON")?;
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-search-benchmark-v1") {
        return Err("installed search benchmark returned the wrong schema".into());
    }
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn verify_installed_search_ui(config: &NativeDeviceConfig, output_dir: &Path) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    fs::create_dir_all(output_dir)?;
    let run_result = (|| -> Result<Value> {
        let initial = read_launcher_status(&session)?;
        if initial.get("catalog_ready").and_then(Value::as_bool) != Some(true)
            || initial
                .get("catalog_games")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                == 0
        {
            return Err("search UI verification requires an existing usable cached catalog".into());
        }
        restart_launcher_with_one_shot_env(
            &session,
            LauncherRestartOptions {
                env_vars: vec![
                    ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                    ("MISTER_LAUNCHER_START_SCREEN".into(), "arcade".into()),
                    ("MISTER_LAUNCHER_START_SYSTEM".into(), "arcade".into()),
                    (
                        "MISTER_LAUNCHER_INPUT_SCRIPT".into(),
                        "left,b,down,a,a,wait:180".into(),
                    ),
                    (
                        "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES".into(),
                        "10".into(),
                    ),
                ],
                timeout_secs: 45,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                ..LauncherRestartOptions::default()
            },
        )?;
        let started = Instant::now();
        let timeout = Duration::from_secs(30);
        loop {
            let status = read_launcher_status(&session)?;
            if search_ui_status_ready(&status) {
                return Ok(json!({
                    "schema": "mister-magik-search-ui-verification-v1",
                    "status": "ready",
                    "query": "A",
                    "results": status["arcade_search_results"],
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }));
            }
            if status.get("arcade_search_status").and_then(Value::as_str) == Some("failed") {
                return Err(format!(
                    "launcher search failed for query {:?}",
                    status.get("arcade_search_query")
                )
                .into());
            }
            if started.elapsed() >= timeout {
                return Err(format!(
                    "launcher search did not reach ready results within {} ms; final status={status}",
                    started.elapsed().as_millis()
                )
                .into());
            }
            thread::sleep(Duration::from_millis(100));
        }
    })();
    let restore_result = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    );
    let summary = match (run_result, restore_result) {
        (Ok(summary), Ok(())) => summary,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("search UI verification cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(format!(
                "{run_error}; search UI verification cleanup failed: {cleanup_error}"
            )
            .into());
        }
    };
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn search_ui_status_ready(status: &Value) -> bool {
    status.get("scene").and_then(Value::as_str) == Some("launcher")
        && status.get("screen").and_then(Value::as_str) == Some("arcade")
        && status.get("arcade_search_active").and_then(Value::as_bool) == Some(true)
        && status.get("arcade_search_status").and_then(Value::as_str) == Some("ready")
        && status.get("arcade_search_query").and_then(Value::as_str) == Some("A")
        && status
            .get("arcade_search_results")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
}

fn search_benchmark_waits_for_catalog(output: &ExecOutput) -> bool {
    let contains = |needle| output.stdout.contains(needle) || output.stderr.contains(needle);
    contains("no such table: game_search_fts")
        || contains("no valid manifest slot")
        || contains("search system arcade is absent")
        || contains("unsupported persisted search schema version")
}

fn profile_installed_catalog_lifecycle(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing")?;
    let boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable")?
        .trim()
        .to_string();
    fs::create_dir_all(output_dir)?;

    let mut lifecycle_log = String::new();
    let mut inspect_log = String::new();
    let run_result = (|| -> Result<Value> {
        exec_checked(
            &session,
            "catalog lifecycle isolated fixture",
            &catalog_lifecycle_prepare_command(),
        )?;
        let started = Instant::now();
        restart_launcher_with_one_shot_env(
            &session,
            LauncherRestartOptions {
                env_vars: catalog_lifecycle_launcher_env(),
                timeout_secs: CATALOG_LIFECYCLE_FIRST_VISIBLE_TIMEOUT_SECS,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                ..LauncherRestartOptions::default()
            },
        )?;
        let mut first_visible_ms = None;
        let (catalog, final_status) = loop {
            let status = read_launcher_status(&session)?;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            if first_visible_ms.is_none()
                && status.get("catalog_ready").and_then(Value::as_bool) == Some(true)
                && status
                    .get("catalog_games")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            {
                first_visible_ms = Some(elapsed_ms);
                lifecycle_log.push_str(&format!(
                    "first_visible elapsed_ms={elapsed_ms} games={}\n",
                    status["catalog_games"]
                ));
                match exec(&session, &catalog_lifecycle_affinity_command(), true) {
                    Ok(affinity) => {
                        lifecycle_log.push_str("first_visible_thread_policy\n");
                        lifecycle_log.push_str(&affinity.stdout);
                        lifecycle_log.push_str(&affinity.stderr);
                    }
                    Err(error) => {
                        lifecycle_log.push_str(&format!("affinity_error={error}\n"));
                    }
                }
            }
            if first_visible_ms.is_none()
                && started.elapsed()
                    >= Duration::from_secs(CATALOG_LIFECYCLE_FIRST_VISIBLE_TIMEOUT_SECS)
            {
                return Err(format!(
                    "launcher catalog did not become first-visible within {} seconds; final status={status}",
                    CATALOG_LIFECYCLE_FIRST_VISIBLE_TIMEOUT_SECS
                )
                .into());
            }

            let inspect = exec(
                &session,
                &catalog_lifecycle_runtime_command("catalog-v3-inspect"),
                true,
            )?;
            if exec_failure_message("catalog lifecycle inspect", &inspect).is_none() {
                inspect_log = inspect.stdout;
                break (parse_catalog_lifecycle_inspect(&inspect_log)?, status);
            }
            if started.elapsed() >= Duration::from_secs(CATALOG_LIFECYCLE_COMPLETE_TIMEOUT_SECS) {
                lifecycle_log.push_str(&inspect.stdout);
                lifecycle_log.push_str(&inspect.stderr);
                return Err(format!(
                    "launcher catalog did not publish a valid manifest within {} seconds; first_visible_ms={first_visible_ms:?}",
                    CATALOG_LIFECYCLE_COMPLETE_TIMEOUT_SECS
                )
                .into());
            }
            thread::sleep(Duration::from_secs(1));
        };
        let complete_ms = started.elapsed().as_millis() as u64;
        lifecycle_log.push_str(&format!(
            "complete elapsed_ms={complete_ms} systems={} games={}\n",
            catalog["systems"].as_array().map_or(0, Vec::len),
            catalog["total_games"]
        ));
        fs::write(
            output_dir.join("launcher-status.json"),
            format!("{}\n", serde_json::to_string_pretty(&final_status)?),
        )?;
        Ok(json!({
            "schema": "mister-magik-installed-benchmark-v1",
            "scenario": "catalog-lifecycle",
            "elapsed_ms": complete_ms,
            "timing": {
                "first_visible_ms": first_visible_ms,
                "complete_ms": complete_ms,
            },
            "catalog": catalog,
            "manifest": parse_manifest_evidence(&manifest),
            "boot_id": boot_id,
            "isolation": {
                "remote_root": CATALOG_LIFECYCLE_REMOTE_DIR,
                "production_paths_redirected": true,
            },
            "output_dir": output_dir,
        }))
    })();

    match exec(&session, &catalog_lifecycle_evidence_command(), true) {
        Ok(diagnostics) => {
            lifecycle_log.push_str(&diagnostics.stdout);
            lifecycle_log.push_str(&diagnostics.stderr);
            if let Some(error) = exec_failure_message("catalog lifecycle evidence", &diagnostics) {
                lifecycle_log.push_str(&format!("evidence_error={error}\n"));
            }
        }
        Err(error) => lifecycle_log.push_str(&format!("evidence_error={error}\n")),
    }
    let evidence_result = (|| -> Result<()> {
        fs::write(output_dir.join("catalog-lifecycle.log"), &lifecycle_log)?;
        fs::write(output_dir.join("catalog-inspect.tsv"), &inspect_log)?;
        Ok(())
    })();

    let restart_result = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            timeout_secs: CATALOG_LIFECYCLE_FIRST_VISIBLE_TIMEOUT_SECS,
            ..LauncherRestartOptions::default()
        },
    );
    let cleanup_result = exec_checked(
        &session,
        "catalog lifecycle isolated cleanup",
        &catalog_lifecycle_cleanup_command(),
    );
    let restore_result = combine_catalog_lifecycle_restore(cleanup_result, restart_result);
    let summary = match (run_result, evidence_result, restore_result) {
        (Ok(summary), Ok(()), Ok(())) => summary,
        (Err(error), _, Ok(())) => return Err(error),
        (Ok(_), Err(error), Ok(())) => return Err(error),
        (Ok(_), _, Err(error)) => {
            return Err(format!("catalog lifecycle benchmark restore failed: {error}").into());
        }
        (Err(run_error), _, Err(restore_error)) => {
            return Err(format!(
                "{run_error}; catalog lifecycle benchmark restore failed: {restore_error}"
            )
            .into());
        }
    };

    drop(session);
    let session = connect_with(&config.connection, 10)?;
    let final_boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable after catalog lifecycle benchmark")?;
    if final_boot_id.trim() != boot_id {
        return Err("device rebooted during the catalog lifecycle benchmark".into());
    }
    let final_manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing after catalog lifecycle benchmark")?;
    if final_manifest != manifest {
        return Err(
            "installed platform manifest changed during catalog lifecycle benchmark".into(),
        );
    }

    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    fs::write(
        output_dir.join("report.md"),
        catalog_lifecycle_report(&summary)?,
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

const LAUNCH_RETURN_GATE_REMOTE: &str = "/tmp/mister-magik/launch-return-benchmark-gate";
const LAUNCH_RETURN_STATE_REMOTE: &str = "/tmp/mister-magik/launcher-return-state.json";
const LAUNCH_RETURN_PROFILE_REMOTE_DIR: &str = "/tmp/mister-magik/launch-return-profile";
const LAUNCH_RETURN_CYCLES: usize = 2;
const LAUNCH_RETURN_BLACK_LIMIT_MS: u64 = 5_000;
const LAUNCH_RETURN_GAME_SETTLE_SECS: u64 = 10;

fn cold_boot_profile_preflight_command() -> String {
    let verify = installed_platform_verify_command(Layout::Development);
    shell_sequence([
        "set -eu",
        verify.as_str(),
        "test ! -e /tmp/mister-magik/reboot-unstable",
        "test ! -e /media/fat/mister-magik/launcher.env",
        "test ! -e /media/fat/mister-magik-dev/launcher.env",
        "test ! -e /tmp/mister-magik/fs-fault-launcher.env",
        "test ! -e /tmp/mister-magik/fs-fault-session",
        "test ! -e /tmp/mister-magik/fs-fault.json",
        "test ! -e /media/fat/mister-magik/rebuild-on-next-boot",
        "test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot",
        "sync",
    ])
}

fn parse_boot_events(text: &str) -> Result<Vec<Value>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str::<Value>(line).map_err(|error| {
                format!("invalid boot event at line {}: {error}", index + 1).into()
            })
        })
        .collect()
}

fn boot_event_us(events: &[Value], name: &str, last: bool) -> Result<u64> {
    let mut matching = events.iter().filter(|event| {
        event.get("event").and_then(Value::as_str) == Some(name)
            && (event.get("ts_boot_us").and_then(Value::as_u64).is_some()
                || event.get("ts_boot_ms").and_then(Value::as_u64).is_some())
    });
    let event = if last {
        matching.next_back()
    } else {
        matching.next()
    }
    .ok_or_else(|| format!("cold-boot event is missing: {name}"))?;
    Ok(event
        .get("ts_boot_us")
        .and_then(Value::as_u64)
        .or_else(|| {
            event
                .get("ts_boot_ms")
                .and_then(Value::as_u64)
                .map(|milliseconds| milliseconds.saturating_mul(1_000))
        })
        .unwrap_or(0))
}

fn parse_magik_startup_events(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            if fields.next()? != "startup_timing" {
                return None;
            }
            let event = fields.next()?;
            let elapsed_us = fields.next()?.strip_suffix("us")?.parse::<u64>().ok()?;
            Some(json!({
                "event": event,
                "elapsed_us": elapsed_us,
                "detail": fields.collect::<Vec<_>>().join("\t"),
            }))
        })
        .collect()
}

fn profile_installed_cold_boot(config: &NativeDeviceConfig, output_dir: &Path) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let session = connect_with(&config.connection, 10)?;
    let installed_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("cold-boot benchmark cannot read the installed Dev manifest")?;
    let main_revision = exact_manifest_field(&installed_manifest, "main_revision", 40)?;
    let main_sha256 = exact_manifest_field(&installed_manifest, "main_sha256", 64)?;
    let boot_id_before = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("cold-boot benchmark cannot read the initial boot id")?;
    exec_checked(
        &session,
        "cold-boot benchmark safety preflight",
        &cold_boot_profile_preflight_command(),
    )?;

    let host_started = Instant::now();
    let issue_started = Instant::now();
    let reboot_mode = issue_reboot(&session, RebootMode::Supervised)?;
    let host_reboot_issue_ms = issue_started.elapsed().as_millis() as u64;
    drop(session);
    if !wait_down_with(&config.connection, 40.0) || wait_up_with(&config.connection, 120.0)? != 0 {
        return Err("device did not complete the cold-boot profile reboot".into());
    }
    wait_authenticated_agent_ready(config, Duration::from_secs(30))
        .map_err(|error| format!("{error:?}"))?;
    let session = connect_with(&config.connection, 10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    wait_delivery_health(&session, "dev", Duration::from_secs(10))?;
    let host_recovery_elapsed_ms = host_started.elapsed().as_millis() as u64;

    let boot_id_after = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("cold-boot benchmark cannot read the final boot id")?;
    if boot_id_after.trim() == boot_id_before.trim() {
        return Err("cold-boot benchmark did not observe a new Linux boot id".into());
    }
    let events_text = remote_read(&session, "/tmp/mister-magik/events.jsonl")
        .filter(|text| !text.trim().is_empty())
        .ok_or("cold-boot benchmark has no Main event log")?;
    let launcher_log = remote_read(&session, "/tmp/mister-magik-slint.log")
        .filter(|text| !text.trim().is_empty())
        .ok_or("cold-boot benchmark has no MagiK launcher log")?;
    let main_status_text = remote_read(&session, MAIN_STATUS_REMOTE)
        .ok_or("cold-boot benchmark has no Main status")?;
    let launcher_status_text = remote_read(&session, SLINT_STATUS_REMOTE)
        .ok_or("cold-boot benchmark has no launcher status")?;
    let main_status: Value = serde_json::from_str(&main_status_text)?;
    let launcher_status: Value = serde_json::from_str(&launcher_status_text)?;
    let agent_diagnostics = agent_request_at(
        &config.agent,
        "diagnostics",
        json!({}),
        Duration::from_secs(10),
    )?
    .response
    .get("result")
    .cloned()
    .ok_or("cold-boot benchmark has no agent diagnostics")?;
    let dmesg = exec(&session, "dmesg", true)?;
    if let Some(message) = exec_failure_message("cold-boot dmesg", &dmesg) {
        return Err(message.into());
    }
    let inittab = remote_read(&session, "/etc/inittab")
        .ok_or("cold-boot benchmark cannot read /etc/inittab")?;
    let boot_analytics =
        remote_read(&session, "/tmp/mister-magik-boot-analytics.tsv").unwrap_or_default();
    let events = parse_boot_events(&events_text)?;
    let startup_events = parse_magik_startup_events(&launcher_log);

    let agent_timeline = agent_diagnostics
        .pointer("/timeline/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let agent_start_us = agent_timeline
        .iter()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("agent_start"))
        .and_then(|event| event.get("uptime_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_mul(1_000);

    let initial_main_us = boot_event_us(&events, "main_process_entry", false)?;
    let final_main_us = boot_event_us(&events, "main_process_entry", true)?;
    let preflight_begin_us = boot_event_us(&events, "launcher_preflight_begin", true)?;
    let preflight_end_us = boot_event_us(&events, "launcher_preflight_end", true)?;
    let launcher_exec_us = boot_event_us(&events, "launcher_exec_begin", true)?;
    let process_start_us = launcher_status
        .get("process_start_monotonic_us")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let first_present_elapsed_us = startup_events
        .iter()
        .find(|event| {
            event.get("event").and_then(Value::as_str) == Some("launcher_first_frame_presented")
        })
        .and_then(|event| event.get("elapsed_us"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let first_present_us = boot_event_us(&events, "launcher_first_frame_presented", true)?;
    let startup_clock_origin_us = first_present_us.saturating_sub(first_present_elapsed_us);
    let ordered = [
        initial_main_us,
        final_main_us,
        preflight_begin_us,
        preflight_end_us,
        launcher_exec_us,
        process_start_us,
        first_present_us,
    ];
    if agent_start_us == 0
        || agent_start_us > initial_main_us
        || ordered[0] == 0
        || first_present_elapsed_us == 0
        || ordered.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(format!("cold-boot timestamps are zero or unordered: {ordered:?}").into());
    }

    let capture = request_framebuffer_png_at_when_latched(&config.agent, Duration::from_secs(3))?;
    validate_visible_launcher_capture(&capture)?;
    fs::write(output_dir.join("boot-rgb565.png"), &capture.png)?;
    fs::write(
        output_dir.join("boot-capture.json"),
        format!("{}\n", serde_json::to_string_pretty(&capture.result)?),
    )?;
    for (name, text) in [
        ("events.jsonl", events_text.as_str()),
        ("launcher.log", launcher_log.as_str()),
        ("main-status.json", main_status_text.as_str()),
        ("launcher-status.json", launcher_status_text.as_str()),
        ("platform-v3.manifest", installed_manifest.as_str()),
        ("dmesg.log", dmesg.stdout.as_str()),
        ("inittab.txt", inittab.as_str()),
        ("boot-analytics.tsv", boot_analytics.as_str()),
    ] {
        fs::write(output_dir.join(name), text)?;
    }
    fs::write(
        output_dir.join("agent-diagnostics.json"),
        format!("{}\n", serde_json::to_string_pretty(&agent_diagnostics)?),
    )?;

    let timeline = json!({
        "agent_start_us": agent_start_us,
        "initial_main_entry_us": initial_main_us,
        "final_main_entry_us": final_main_us,
        "preflight_begin_us": preflight_begin_us,
        "preflight_end_us": preflight_end_us,
        "launcher_exec_us": launcher_exec_us,
        "magik_process_start_us": process_start_us,
        "first_launcher_present_us": first_present_us,
        "first_launcher_present_resolution_us": 10_000,
        "magik_startup_clock_origin_us": startup_clock_origin_us,
        "main_events": events,
        "magik_startup_events": startup_events,
        "agent_timeline": agent_timeline,
    });
    let phases = json!({
        "linux_boot_to_agent_start_us": agent_start_us,
        "agent_start_to_initial_main_us": initial_main_us.saturating_sub(agent_start_us),
        "linux_boot_to_initial_main_us": initial_main_us,
        "initial_main_to_final_main_us": final_main_us.saturating_sub(initial_main_us),
        "final_main_to_preflight_us": preflight_begin_us.saturating_sub(final_main_us),
        "preflight_us": preflight_end_us.saturating_sub(preflight_begin_us),
        "preflight_to_launcher_exec_us": launcher_exec_us.saturating_sub(preflight_end_us),
        "launcher_exec_to_magik_process_us": process_start_us.saturating_sub(launcher_exec_us),
        "magik_process_to_startup_clock_us": startup_clock_origin_us.saturating_sub(process_start_us),
        "startup_clock_to_first_present_us": first_present_us.saturating_sub(startup_clock_origin_us),
        "magik_process_to_first_present_us": first_present_us.saturating_sub(process_start_us),
        "linux_boot_to_first_present_us": first_present_us,
    });
    let launcher_ready = main_status.get("launcher_state").and_then(Value::as_str)
        == Some("LauncherActive")
        && launcher_status
            .get("input_enabled")
            .and_then(Value::as_bool)
            == Some(true);
    let summary = json!({
        "schema": "mister-magik-cold-boot-benchmark-v1",
        "scenario": "cold-boot",
        "timing_class": "device-monotonic-instrumented-installed-dev",
        "main_revision": main_revision,
        "main_sha256": main_sha256,
        "boot_id_before": boot_id_before.trim(),
        "boot_id_after": boot_id_after.trim(),
        "reboot_mode": reboot_mode,
        "host_reboot_issue_ms": host_reboot_issue_ms,
        "host_recovery_elapsed_ms": host_recovery_elapsed_ms,
        "launcher_ready": launcher_ready,
        "screen": launcher_status.get("screen"),
        "effective_view": launcher_status.get("effective_view"),
        "capture_verified": true,
        "capture_file": "boot-rgb565.png",
        "capture_metadata_file": "boot-capture.json",
        "phases": phases,
        "timeline": timeline,
    });
    fs::write(
        output_dir.join("timeline.json"),
        format!("{}\n", serde_json::to_string_pretty(&timeline)?),
    )?;
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn exact_manifest_field(manifest: &str, field: &str, length: usize) -> Result<String> {
    let values = manifest
        .lines()
        .filter_map(|line| line.strip_prefix(&format!("{field}=")))
        .collect::<Vec<_>>();
    if values.len() != 1
        || values[0].len() != length
        || !values[0]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("installed Dev manifest has invalid {field}").into());
    }
    Ok(values[0].into())
}

fn legacy_launcher_restart_cleanup_command() -> &'static str {
    "set -eu; now=$(cut -d. -f1 /proc/uptime); ticks=$(getconf CLK_TCK 2>/dev/null || echo 100); killed=0; for proc in /proc/[0-9]*; do pid=${proc##*/}; test \"$pid\" != \"$$\" || continue; target=$(readlink \"$proc/fd/8\" 2>/dev/null || true); test \"$target\" = /tmp/mister-magik/command-operation.lock || continue; command=$(tr '\\000' ' ' < \"$proc/cmdline\" 2>/dev/null || true); case \"$command\" in *mister_magik_restart_launcher*) ;; *) continue ;; esac; start=$(awk '{print $22}' \"$proc/stat\" 2>/dev/null || echo 0); age=$((now - start / ticks)); test \"$age\" -ge 30 || continue; kill \"$pid\"; killed=$((killed + 1)); done; test \"$killed\" -eq 0 || sleep 1; printf 'legacy_launcher_restart_cleanup_tsv\\tkilled=%s\\n' \"$killed\""
}

fn profile_installed_launch_return(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    force_capsule_miss: bool,
) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    fs::create_dir_all(output_dir)?;
    let _signal_guard = ScreensaverProfileSignalGuard::install();
    let mut cycles = Vec::new();
    let installed_manifest = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("launch-return benchmark cannot read the installed Dev manifest")?;
    let main_revision = exact_manifest_field(&installed_manifest, "main_revision", 40)?;
    let main_sha256 = exact_manifest_field(&installed_manifest, "main_sha256", 64)?;
    let magik_revision = exact_manifest_field(&installed_manifest, "magik_revision", 40)?;
    let gui_sha256 = exact_manifest_field(&installed_manifest, "gui_sha256", 64)?;
    let run_result = (|| -> Result<Value> {
        exec_checked(
            &session,
            "launch-return legacy restart cleanup",
            legacy_launcher_restart_cleanup_command(),
        )?;
        exec_checked(
            &session,
            "launch-return benchmark preflight cleanup",
            &remove_files_command(&[
                DEVELOPMENT_LAUNCHER_ENV_REMOTE,
                LAUNCH_RETURN_GATE_REMOTE,
                LAUNCH_RETURN_STATE_REMOTE,
            ]),
        )?;
        exec_checked(
            &session,
            "launch-return benchmark stale profile cleanup",
            &format!("rm -rf {}", sh(LAUNCH_RETURN_PROFILE_REMOTE_DIR)),
        )?;
        wait_delivery_health(&session, "dev", Duration::from_secs(10))?;
        restart_launcher_with_one_shot_env(
            &session,
            LauncherRestartOptions {
                env_vars: vec![
                    ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                    ("MISTER_LAUNCHER_START_SCREEN".into(), "arcade".into()),
                    ("MISTER_LAUNCHER_START_SYSTEM".into(), "arcade".into()),
                    ("MISTER_ARCADE_SELECTED_INDEX".into(), "128".into()),
                    ("MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED".into(), "1".into()),
                    (
                        "MISTER_MAGIK_TEST_AUTO_LAUNCH_GATE".into(),
                        LAUNCH_RETURN_GATE_REMOTE.into(),
                    ),
                ],
                timeout_secs: 45,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                ..LauncherRestartOptions::default()
            },
        )?;
        let initial_status = read_launcher_status(&session)?;
        let mut previous_launcher_pid = initial_status
            .get("pid")
            .and_then(Value::as_i64)
            .ok_or("initial launcher status has no pid")?;
        exec_checked(
            &session,
            "release launch-return initial launch gate",
            &format!("set -eu; : > {}", sh(LAUNCH_RETURN_GATE_REMOTE)),
        )?;
        let mut expected = wait_launch_return_state(&session, None, Duration::from_secs(20))?;

        for cycle_index in 0..LAUNCH_RETURN_CYCLES {
            wait_launch_return_duration(Duration::from_secs(LAUNCH_RETURN_GAME_SETTLE_SECS))?;
            let cycle_number = cycle_index + 1;
            let remote_profile_dir = LAUNCH_RETURN_PROFILE_REMOTE_DIR;
            let remote_svg = format!("{remote_profile_dir}/cycle-{cycle_number}.svg");
            let remote_folded = format!("{remote_profile_dir}/cycle-{cycle_number}.folded");
            let remote_frames = format!("{remote_profile_dir}/cycle-{cycle_number}-frames.tsv");
            let remote_complete = format!("{remote_profile_dir}/cycle-{cycle_number}-profile.json");
            let mut return_env = vec![
                ("MISTER_PPROF".into(), "1".into()),
                ("MISTER_PPROF_TRIGGER".into(), "launch-return".into()),
                ("MISTER_PPROF_HZ".into(), "999".into()),
                ("MISTER_PPROF_OUT".into(), remote_svg.clone()),
                ("MISTER_PPROF_FOLDED_OUT".into(), remote_folded.clone()),
                ("MISTER_PPROF_COMPLETE".into(), remote_complete.clone()),
                ("MISTER_PROFILE".into(), "full".into()),
                ("MISTER_BOOT_ANALYTICS".into(), "1".into()),
                (
                    "MISTER_BOOT_FRAME_PROFILE_FILE".into(),
                    remote_frames.clone(),
                ),
                ("MISTER_BOOT_FRAME_PROFILE_FRAMES".into(), "240".into()),
            ];
            if cycle_index + 1 < LAUNCH_RETURN_CYCLES {
                return_env.extend([
                    (
                        "MISTER_LAUNCHER_INPUT_SCRIPT".into(),
                        // Leave the returned launcher alive long enough for the 240-frame
                        // profile and its artifacts to be copied before the next core handoff.
                        // Let the one-row motion reach its canonical settled scroll
                        // position before launch state is captured. Twelve frames can
                        // catch the final interpolation pixels (for example 6190
                        // instead of the settled 6192) and falsely fail exact return.
                        "wait:360,down,wait:60,a".into(),
                    ),
                    (
                        "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES".into(),
                        "1".into(),
                    ),
                ]);
            }
            exec_checked(
                &session,
                "prepare launch-return profile directory",
                &format!("set -eu; mkdir -p {}", sh(remote_profile_dir)),
            )?;
            put_bytes(
                &session,
                DEVELOPMENT_LAUNCHER_ENV_REMOTE,
                one_shot_launcher_env_text(&return_env, DEVELOPMENT_LAUNCHER_ENV_REMOTE).as_bytes(),
            )?;
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-pre-launch-state.json")),
                format!("{}\n", serde_json::to_string_pretty(&expected)?),
            )?;

            let capsule_fault_injected = force_capsule_miss && cycle_number == 2;
            if capsule_fault_injected {
                exec_checked(
                    &session,
                    "remove launch-return capsule for fixed fallback cycle",
                    &remove_files_command(&[
                        RETURN_CATALOG_CAPSULE_REMOTE,
                        "/tmp/mister-magik/launcher-return-catalog.json.tmp",
                    ]),
                )?;
            }

            let host_poll_started = Instant::now();
            let return_action =
                request_magik_benchmark_action(&config.agent, "return-to-launcher")?;
            let status =
                wait_launch_return_ready(&session, previous_launcher_pid, Duration::from_secs(8))?;
            let host_poll_elapsed_ms = host_poll_started.elapsed().as_millis() as u64;
            let expected_index = expected
                .get("game_index")
                .and_then(Value::as_u64)
                .ok_or("launch return state has no game_index")?;
            let expected_path = expected
                .get("game_path")
                .and_then(Value::as_str)
                .ok_or("launch return state has no game_path")?
                .to_string();
            let selected = status
                .get("arcade_selected")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            let visual_index = status
                .get("arcade_visual_index")
                .and_then(Value::as_f64)
                .unwrap_or(f64::NAN);
            let preview_state = status
                .get("preview_cache_state")
                .and_then(Value::as_str)
                .unwrap_or("missing");
            let preview_expected = status
                .get("selected_game_has_preview")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let preview_verified = preview_state == "exact" || !preview_expected;
            let expected_collection = expected
                .get("collection_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let selected_path = status
                .get("selected_game_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let selected_collection = status
                .get("active_collection_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let expected_scroll_y = expected
                .get("scroll_y")
                .and_then(Value::as_i64)
                .unwrap_or(expected_index as i64 * 48);
            let scroll_y = status
                .get("arcade_scroll_y")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MIN);
            let request_us = return_action
                .get("request_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let acknowledge_us = return_action
                .get("acknowledged_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let process_us = status
                .get("process_start_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let context_us = status
                .get("exact_context_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let preview_us = status
                .get("preview_ready_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let present_us = status
                .get("first_correct_present_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let monotonic_ordered = request_us > 0
                && request_us <= acknowledge_us
                && acknowledge_us <= process_us
                && process_us <= context_us
                && context_us <= preview_us
                && preview_us <= present_us;
            if !monotonic_ordered {
                return Err(format!(
                    "launch-return cycle {cycle_number} has zero or unordered device-monotonic timestamps request={request_us} acknowledge={acknowledge_us} process={process_us} context={context_us} preview={preview_us} present={present_us}"
                )
                .into());
            }
            let total_return_us = present_us - request_us;
            let visible_black_ms = total_return_us.saturating_add(999) / 1_000;
            let restored = status.get("return_screen").and_then(Value::as_str) == Some("arcade")
                && selected == expected_index
                && (visual_index - expected_index as f64).abs() < 0.01
                && scroll_y == expected_scroll_y
                && selected_path == expected_path
                && selected_collection == expected_collection
                && preview_verified
                && visible_black_ms < LAUNCH_RETURN_BLACK_LIMIT_MS;
            let capture =
                request_framebuffer_png_at_when_latched(&config.agent, Duration::from_secs(3))?;
            validate_visible_launcher_capture(&capture)?;
            let capture_file = format!("cycle-{cycle_number}-rgb565.png");
            let capture_metadata_file = format!("cycle-{cycle_number}-capture.json");
            fs::write(output_dir.join(&capture_file), &capture.png)?;
            fs::write(
                output_dir.join(&capture_metadata_file),
                format!("{}\n", serde_json::to_string_pretty(&capture.result)?),
            )?;
            let profile_metadata_text =
                wait_launch_return_artifact(&session, &remote_complete, Duration::from_secs(5))?;
            let profile_metadata: Value = serde_json::from_str(profile_metadata_text.trim())?;
            let sample_hits = profile_metadata
                .get("sample_hits")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let sample_stacks = profile_metadata
                .get("sample_stacks")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if profile_metadata.get("state").and_then(Value::as_str) != Some("complete")
                || sample_hits <= 0
                || sample_stacks == 0
            {
                return Err(format!(
                    "launch-return cycle {cycle_number} produced no valid CPU samples"
                )
                .into());
            }
            let folded = remote_read(&session, &remote_folded)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| {
                    format!("launch-return profile artifact is missing: {remote_folded}")
                })?;
            let resolved_application_symbols =
                folded.contains("mister_magik") || folded.contains("slint::");
            if !resolved_application_symbols {
                return Err(format!(
                    "launch-return cycle {cycle_number} folded stacks have no resolved application symbols"
                )
                .into());
            }
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-stacks.folded")),
                &folded,
            )?;
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-profile.json")),
                format!("{}\n", serde_json::to_string_pretty(&profile_metadata)?),
            )?;
            let frame_profile =
                wait_launch_return_artifact(&session, &remote_frames, Duration::from_secs(6))?;
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-frames.tsv")),
                frame_profile,
            )?;
            let artifact = remote_read(&session, &remote_svg)
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| {
                    format!("launch-return profile artifact is missing: {remote_svg}")
                })?;
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-flamegraph.svg")),
                artifact,
            )?;
            for (remote, local) in [
                (
                    "/tmp/mister-magik/events.jsonl",
                    format!("cycle-{cycle_number}-events.jsonl"),
                ),
                (
                    "/tmp/mister-magik-slint.log",
                    format!("cycle-{cycle_number}-launcher.log"),
                ),
            ] {
                let artifact = remote_read(&session, remote)
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| format!("launch-return log artifact is missing: {remote}"))?;
                if remote.ends_with("events.jsonl") {
                    let presented_home_after_request = artifact.lines().any(|line| {
                        let Ok(event) = serde_json::from_str::<Value>(line) else {
                            return false;
                        };
                        event.get("event").and_then(Value::as_str)
                            == Some("launcher_first_frame_presented")
                            && event
                                .get("ts_boot_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or(0)
                                .saturating_mul(1_000)
                                >= process_us
                            && event
                                .get("detail")
                                .and_then(Value::as_str)
                                .is_some_and(|detail| detail.contains("screen=home"))
                    });
                    if presented_home_after_request {
                        return Err(format!(
                            "launch-return cycle {cycle_number} presented Home before exact Arcade restoration"
                        )
                        .into());
                    }
                }
                fs::write(output_dir.join(local), artifact)?;
            }
            let mut cycle = json!({
                "cycle": cycle_number,
                "capsule_fault_injected": capsule_fault_injected,
                "expected_game_path": &expected_path,
                "expected_game_index": expected_index,
                "selected_game_index": selected,
                "visual_index": visual_index,
                "expected_scroll_y": expected_scroll_y,
                "scroll_y": scroll_y,
                "expected_collection_id": expected_collection,
                "selected_collection_id": selected_collection,
                "selected_game_path": selected_path,
                "preview_cache_state": preview_state,
                "preview_expected": preview_expected,
                "preview_verified": preview_verified,
                "black_interval_ms": visible_black_ms,
                "visible_black_us": total_return_us,
                "host_poll_elapsed_ms": host_poll_elapsed_ms,
                "return_screen": status.get("return_screen"),
                "startup_mode": status.get("startup_mode"),
                "return_source": status.get("return_source"),
                "return_phase": status.get("return_phase"),
            });
            let timing_and_artifacts = json!({
                "request_monotonic_us": request_us,
                "acknowledged_monotonic_us": acknowledge_us,
                "process_start_monotonic_us": process_us,
                "exact_context_monotonic_us": context_us,
                "preview_ready_monotonic_us": preview_us,
                "first_correct_present_monotonic_us": present_us,
                "command_to_process_us": process_us.saturating_sub(request_us),
                "process_to_context_us": context_us.saturating_sub(process_us),
                "context_to_preview_us": preview_us.saturating_sub(context_us),
                "preview_to_present_us": present_us.saturating_sub(preview_us),
                "total_return_us": total_return_us,
                "capture_file": capture_file,
                "capture_metadata_file": capture_metadata_file,
                "flamegraph_file": format!("cycle-{cycle_number}-flamegraph.svg"),
                "folded_stacks_file": format!("cycle-{cycle_number}-stacks.folded"),
                "cpu_profile_file": format!("cycle-{cycle_number}-profile.json"),
                "cpu_sample_hits": sample_hits,
                "cpu_sample_stacks": sample_stacks,
                "resolved_application_symbols": resolved_application_symbols,
                "frame_profile_file": format!("cycle-{cycle_number}-frames.tsv"),
                "timeline_file": format!("cycle-{cycle_number}-timeline.json"),
                "restored": restored,
            });
            cycle
                .as_object_mut()
                .ok_or("launch-return cycle summary is not an object")?
                .extend(
                    timing_and_artifacts
                        .as_object()
                        .ok_or("launch-return timing summary is not an object")?
                        .clone(),
                );
            fs::write(
                output_dir.join(format!("cycle-{}-status.json", cycle_index + 1)),
                format!("{}\n", serde_json::to_string_pretty(&status)?),
            )?;
            fs::write(
                output_dir.join(format!("cycle-{cycle_number}-timeline.json")),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&json!({
                        "schema": "mister-magik-launch-return-timeline-v1",
                        "request_monotonic_us": request_us,
                        "acknowledged_monotonic_us": acknowledge_us,
                        "process_start_monotonic_us": process_us,
                        "exact_context_monotonic_us": context_us,
                        "preview_ready_monotonic_us": preview_us,
                        "first_correct_present_monotonic_us": present_us,
                    }))?
                ),
            )?;
            cycles.push(cycle);
            if !restored {
                return Err(format!(
                    "launch-return cycle {} did not restore within budget: status={status}",
                    cycle_index + 1
                )
                .into());
            }
            previous_launcher_pid = status
                .get("pid")
                .and_then(Value::as_i64)
                .ok_or("returned launcher status has no pid")?;
            if cycle_index + 1 < LAUNCH_RETURN_CYCLES {
                expected = wait_launch_return_state(
                    &session,
                    Some(&expected_path),
                    Duration::from_secs(20),
                )?;
            }
        }

        let latency_values = |field: &str| {
            let mut values = cycles
                .iter()
                .filter_map(|cycle| cycle.get(field).and_then(Value::as_u64))
                .collect::<Vec<_>>();
            values.sort_unstable();
            let median_us = if values.is_empty() {
                0
            } else {
                let lower = values[(values.len() - 1) / 2];
                let upper = values[values.len() / 2];
                ((u128::from(lower) + u128::from(upper)) / 2) as u64
            };
            json!({
                "min_us": values.first().copied().unwrap_or(0),
                "median_us": median_us,
                "max_us": values.last().copied().unwrap_or(0),
            })
        };
        Ok(json!({
            "schema": "mister-magik-launch-return-benchmark-v3",
            "scenario": if force_capsule_miss { "launch-return-fallback" } else { "launch-return" },
            "timing_class": "instrumented-installed-dev-symbols",
            "main_revision": &main_revision,
            "main_sha256": &main_sha256,
            "magik_revision": &magik_revision,
            "gui_sha256": &gui_sha256,
            "cpu_profile_hz": 999,
            "cycles": cycles.clone(),
            "latency": {
                "command_to_process": latency_values("command_to_process_us"),
                "process_to_context": latency_values("process_to_context_us"),
                "context_to_preview": latency_values("context_to_preview_us"),
                "preview_to_present": latency_values("preview_to_present_us"),
                "total_return": latency_values("total_return_us"),
            },
            "black_interval_limit_ms": LAUNCH_RETURN_BLACK_LIMIT_MS,
            "game_settle_secs": LAUNCH_RETURN_GAME_SETTLE_SECS,
        }))
    })();

    for (remote, local) in [
        ("/tmp/mister-magik/events.jsonl", "events.jsonl"),
        ("/tmp/mister-magik-slint.log", "launcher.log"),
        ("/tmp/mister-magik/main-status.json", "main-status.json"),
    ] {
        if let Some(text) = remote_read(&session, remote) {
            let _ = fs::write(output_dir.join(local), text);
        }
    }
    for cycle in 1..=LAUNCH_RETURN_CYCLES {
        for (remote, local) in [
            (
                format!("{LAUNCH_RETURN_PROFILE_REMOTE_DIR}/cycle-{cycle}.svg"),
                format!("cycle-{cycle}-flamegraph.svg"),
            ),
            (
                format!("{LAUNCH_RETURN_PROFILE_REMOTE_DIR}/cycle-{cycle}.folded"),
                format!("cycle-{cycle}-stacks.folded"),
            ),
            (
                format!("{LAUNCH_RETURN_PROFILE_REMOTE_DIR}/cycle-{cycle}-frames.tsv"),
                format!("cycle-{cycle}-frames.tsv"),
            ),
            (
                format!("{LAUNCH_RETURN_PROFILE_REMOTE_DIR}/cycle-{cycle}-profile.json"),
                format!("cycle-{cycle}-profile.json"),
            ),
        ] {
            if !output_dir.join(&local).is_file()
                && let Some(text) = remote_read(&session, &remote)
                && !text.trim().is_empty()
            {
                let _ = fs::write(output_dir.join(local), text);
            }
        }
    }
    let cleanup_result = restore_installed_launch_return(&config.agent, &session);
    let summary = match (run_result, cleanup_result) {
        (Ok(summary), Ok(())) => summary,
        (Err(error), Ok(())) => {
            let failure = json!({
                "schema": "mister-magik-launch-return-benchmark-v3",
                "scenario": if force_capsule_miss { "launch-return-fallback" } else { "launch-return" },
                "timing_class": "instrumented-installed-dev-symbols",
                "main_revision": &main_revision,
                "main_sha256": &main_sha256,
                "magik_revision": &magik_revision,
                "gui_sha256": &gui_sha256,
                "cpu_profile_hz": 999,
                "cycles": cycles,
                "black_interval_limit_ms": LAUNCH_RETURN_BLACK_LIMIT_MS,
                "game_settle_secs": LAUNCH_RETURN_GAME_SETTLE_SECS,
                "error": error.to_string(),
            });
            fs::write(
                output_dir.join("summary.json"),
                format!("{}\n", serde_json::to_string_pretty(&failure)?),
            )?;
            return Err(error);
        }
        (Ok(_), Err(error)) => {
            return Err(format!("launch-return benchmark cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(format!(
                "{run_error}; launch-return benchmark cleanup failed: {cleanup_error}"
            )
            .into());
        }
    };
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn wait_launch_return_state(
    session: &Session,
    previous_path: Option<&str>,
    timeout: Duration,
) -> Result<Value> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if screensaver_profile_interrupted() {
            return Err("launch-return benchmark interrupted".into());
        }
        if let Some(text) = remote_read(session, LAUNCH_RETURN_STATE_REMOTE)
            && let Ok(state) = serde_json::from_str::<Value>(&text)
            && state.get("game_path").and_then(Value::as_str) != previous_path
        {
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "launcher did not write a new return state within {} ms",
        timeout.as_millis()
    )
    .into())
}

fn wait_launch_return_ready(
    session: &Session,
    previous_pid: i64,
    timeout: Duration,
) -> Result<Value> {
    let started = Instant::now();
    let mut last_status = Value::Null;
    while started.elapsed() < timeout {
        if screensaver_profile_interrupted() {
            return Err("launch-return benchmark interrupted".into());
        }
        if let Ok(status) = read_launcher_status(session) {
            let new_process = status.get("pid").and_then(Value::as_i64) != Some(previous_pid);
            let return_startup =
                status.get("startup_mode").and_then(Value::as_str) == Some("return_from_game");
            let input_enabled = status.get("input_enabled").and_then(Value::as_bool) == Some(true);
            // Measure black until the first exact presented frame. A validated capsule
            // may keep the session alive afterward so later authoritative catalog
            // publications can reapply the same position; that background phase is not
            // additional black time and must not delay capture of the visible result.
            let exact_return_presented = status
                .get("first_correct_present_monotonic_us")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0;
            if new_process && return_startup && input_enabled && exact_return_presented {
                return Ok(status);
            }
            last_status = status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "launcher return did not become input-ready within {} ms; last_status={last_status}",
        timeout.as_millis()
    )
    .into())
}

fn wait_launch_return_artifact(session: &Session, path: &str, timeout: Duration) -> Result<String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if screensaver_profile_interrupted() {
            return Err("launch-return benchmark interrupted".into());
        }
        if let Some(text) = remote_read(session, path)
            && !text.trim().is_empty()
        {
            return Ok(text);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "launch-return artifact did not become readable within {} ms: {path}",
        timeout.as_millis()
    )
    .into())
}

fn wait_launch_return_duration(duration: Duration) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < duration {
        if screensaver_profile_interrupted() {
            return Err("launch-return benchmark interrupted".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn request_magik_benchmark_action(endpoint: &AgentEndpoint, action: &str) -> Result<Value> {
    let status = agent_request_at(
        endpoint,
        "magik",
        json!({"action": "status"}),
        Duration::from_secs(5),
    )?;
    let expected_generation = status
        .response
        .pointer("/result/files/main_status/main_generation")
        .and_then(Value::as_u64)
        .ok_or("agent Main status missing generation")?;
    let operation_id = format!(
        "launch-return-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    );
    let reply = agent_request_at(
        endpoint,
        "magik",
        json!({
            "action": action,
            "operation_id": operation_id,
            "expected_generation": expected_generation,
            "target": Value::Null,
        }),
        Duration::from_secs(8),
    )?;
    Ok(reply.response.get("result").cloned().unwrap_or(Value::Null))
}

fn restore_installed_launch_return(endpoint: &AgentEndpoint, session: &Session) -> Result<()> {
    let remove = exec_checked(
        session,
        "launch-return benchmark cleanup",
        &remove_files_command(&[
            DEVELOPMENT_LAUNCHER_ENV_REMOTE,
            LAUNCH_RETURN_GATE_REMOTE,
            LAUNCH_RETURN_STATE_REMOTE,
        ]),
    );
    remove?;
    exec_checked(
        session,
        "launch-return benchmark profile cleanup",
        &format!("rm -rf {}", sh(LAUNCH_RETURN_PROFILE_REMOTE_DIR)),
    )?;
    let main_status = remote_read(session, MAIN_STATUS_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    if launch_return_cleanup_needs_active_restart(&main_status) {
        return launcher_restart(
            session,
            &LauncherRestartOptions {
                clear_env: true,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                timeout_secs: 45,
                ..LauncherRestartOptions::default()
            },
        );
    }
    request_magik_benchmark_action(endpoint, "return-to-launcher")
        .map_err(|error| format!("launch-return cleanup could not return through Main: {error}"))?;
    wait_launcher_ready(session, Instant::now(), Duration::from_secs(45)).map(|_| ())
}

fn launch_return_cleanup_needs_active_restart(main_status: &Value) -> bool {
    main_status.get("launcher_state").and_then(Value::as_str) == Some("LauncherActive")
}

fn catalog_lifecycle_prepare_command() -> String {
    format!(
        "set -eu; root={root}; rm -rf \"$root\"; mkdir -p \"$root\"; test ! -e /media/fat/mister-magik/launcher.env; test ! -e /media/fat/mister-magik-dev/launcher.env; test ! -e /tmp/mister-magik/fs-fault-launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /tmp/mister-magik/fs-fault.json; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot",
        root = sh(CATALOG_LIFECYCLE_REMOTE_DIR),
    )
}

fn catalog_lifecycle_runtime_command(subcommand: &str) -> String {
    let root = CATALOG_LIFECYCLE_REMOTE_DIR;
    format!(
        "env MISTER_SHARDED_CATALOG_DIR={catalog} MISTER_LIBRARY_SQLITE={library} MISTER_ARCADE_BOOTSTRAP_INDEX={bootstrap} MISTER_LIBRARY_REFRESH_LOCK={refresh_lock} MISTER_CATALOG_BUILDER_LOCK={builder_lock} MISTER_CATALOG_READY_SNAPSHOT={ready_snapshot} MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1 /media/fat/mister-magik-dev/mister-magik-fb {subcommand}",
        catalog = sh(&format!("{root}/catalog-v3")),
        library = sh(&format!("{root}/library.sqlite3")),
        bootstrap = sh(&format!("{root}/arcade-bootstrap.nav.lz4b")),
        refresh_lock = sh(&format!("{root}/library-refresh.lock")),
        builder_lock = sh(&format!("{root}/catalog-builder.lock")),
        ready_snapshot = sh(&format!("{root}/catalog-ready.snapshot")),
    )
}

fn catalog_lifecycle_launcher_env() -> Vec<(String, String)> {
    let root = CATALOG_LIFECYCLE_REMOTE_DIR;
    vec![
        ("MISTER_CATALOG_REFRESH".into(), "force".into()),
        (
            "MISTER_SHARDED_CATALOG_DIR".into(),
            format!("{root}/catalog-v3"),
        ),
        (
            "MISTER_LIBRARY_SQLITE".into(),
            format!("{root}/library.sqlite3"),
        ),
        (
            "MISTER_ARCADE_BOOTSTRAP_INDEX".into(),
            format!("{root}/arcade-bootstrap.nav.lz4b"),
        ),
        (
            "MISTER_LIBRARY_REFRESH_LOCK".into(),
            format!("{root}/library-refresh.lock"),
        ),
        (
            "MISTER_CATALOG_BUILDER_LOCK".into(),
            format!("{root}/catalog-builder.lock"),
        ),
        (
            "MISTER_CATALOG_READY_SNAPSHOT".into(),
            format!("{root}/catalog-ready.snapshot"),
        ),
        (
            "MISTER_CATALOG_DIAGNOSTICS_DIR".into(),
            format!("{root}/diagnostics"),
        ),
        (
            "MISTER_LAUNCHER_INPUT_SCRIPT".into(),
            catalog_lifecycle_input_script(),
        ),
        (
            "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES".into(),
            "1".into(),
        ),
    ]
}

fn catalog_lifecycle_input_script() -> String {
    std::iter::repeat_n("down,up,wait:600", 150)
        .collect::<Vec<_>>()
        .join(",")
}

const NAVIGATION_TRANSITION_PROFILE_SECS: u64 = 22;
const NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR: &str =
    "/tmp/mister-magik/navigation-transition-profile";

fn profile_installed_navigation_transitions(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let run_result = (|| -> Result<String> {
        let summary = profile_installed_navigation_transition_run(config, output_dir)?;
        serde_json::to_string(&summary).map_err(Into::into)
    })();
    let restore_result = restore_installed_navigation_transition_profile(config);
    match (run_result, restore_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => {
            Err(format!("navigation transition profile cleanup failed: {error}").into())
        }
        (Err(run), Err(cleanup)) => {
            Err(format!("{run}; navigation transition profile cleanup failed: {cleanup}").into())
        }
    }
}

fn profile_installed_navigation_transition_run(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<Value> {
    fs::create_dir_all(output_dir)?;
    let remote_svg = format!("{NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR}/profile.svg");
    let remote_folded = format!("{NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR}/profile.folded");
    let remote_complete = format!("{NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR}/profile.json");
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "reset navigation transition profile artifacts",
        &format!(
            "set -eu; mkdir -p {0}; rm -f {1} {2} {3}",
            sh(NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR),
            sh(&remote_svg),
            sh(&remote_folded),
            sh(&remote_complete)
        ),
    )?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                (
                    "MISTER_LAUNCHER_INPUT_SCRIPT".into(),
                    "wait:120,a,wait:120,b,wait:90,right,a,wait:120,a,wait:30,a,wait:120,b,wait:120,b,wait:30,b,wait:120".into(),
                ),
                (
                    "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES".into(),
                    "1".into(),
                ),
                ("MISTER_PPROF".into(), "1".into()),
                (
                    "MISTER_PPROF_TRIGGER".into(),
                    "navigation-transitions".into(),
                ),
                (
                    "MISTER_PPROF_DURATION_SECS".into(),
                    NAVIGATION_TRANSITION_PROFILE_SECS.to_string(),
                ),
                ("MISTER_PPROF_HZ".into(), "99".into()),
                ("MISTER_PPROF_OUT".into(), remote_svg.clone()),
                ("MISTER_PPROF_FOLDED_OUT".into(), remote_folded.clone()),
                ("MISTER_PPROF_COMPLETE".into(), remote_complete.clone()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);
    let telemetry = agent_telemetry_until_screensaver_profile_complete(
        &config.agent,
        Duration::from_secs(NAVIGATION_TRANSITION_PROFILE_SECS + 20),
    )?;
    let session = connect_with(&config.connection, 10)?;
    let metadata = remote_read(&session, &remote_complete)
        .ok_or("navigation transition profile completion metadata is missing")?;
    let metadata_value: Value = serde_json::from_str(metadata.trim())?;
    if metadata_value.get("schema").and_then(Value::as_str)
        != Some("mister-magik-navigation-transitions-pprof-v1")
        || metadata_value.get("state").and_then(Value::as_str) != Some("complete")
        || metadata_value
            .get("sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            <= 0
    {
        return Err("navigation transition profile produced no CPU samples".into());
    }
    let svg = remote_read(&session, &remote_svg)
        .filter(|text| !text.is_empty())
        .ok_or("navigation transition profile SVG is missing")?;
    let folded = remote_read(&session, &remote_folded)
        .filter(|text| !text.is_empty())
        .ok_or("navigation transition folded stacks are missing")?;
    fs::write(output_dir.join("flamegraph.svg"), svg)?;
    fs::write(output_dir.join("stacks.folded"), folded)?;
    fs::write(
        output_dir.join("profile.json"),
        format!("{}\n", serde_json::to_string_pretty(&metadata_value)?),
    )?;
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(
        output_dir.join("telemetry.jsonl"),
        format!("{telemetry_text}\n"),
    )?;
    summarize_navigation_transition_profile(output_dir, &telemetry, metadata_value)
}

fn restore_installed_navigation_transition_profile(config: &NativeDeviceConfig) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    let cleanup = format!(
        "rm -f {env} /tmp/mister-magik/realtime-frame-analytics; rm -rf {profiles}",
        env = sh(DEVELOPMENT_LAUNCHER_ENV_REMOTE),
        profiles = sh(NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR),
    );
    exec_checked(&session, "navigation transition profile cleanup", &cleanup)?;
    launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    )?;
    exec_checked(
        &session,
        "navigation transition profile final cleanup",
        &format!(
            "{cleanup}; test ! -e {env}; test ! -e {profiles}",
            env = sh(DEVELOPMENT_LAUNCHER_ENV_REMOTE),
            profiles = sh(NAVIGATION_TRANSITION_PROFILE_REMOTE_DIR),
        ),
    )?;
    Ok(())
}

fn summarize_navigation_transition_profile(
    output_dir: &Path,
    telemetry: &[Value],
    cpu_profile: Value,
) -> Result<Value> {
    use std::fmt::Write as _;

    let mut frames = BTreeMap::<u64, Value>::new();
    for sample in telemetry {
        let Some(recent) = sample
            .pointer("/launcher/frame_budget/recent_frames")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for frame in recent {
            let frame_id = frame_u64(frame, "frame");
            if frame_id > 0 {
                frames.insert(frame_id, frame.clone());
            }
        }
    }
    let transition_frames = frames
        .values()
        .filter(|frame| frame_u64(frame, "navigation_transition_us") > 0)
        .collect::<Vec<_>>();
    if transition_frames.is_empty() {
        return Err("navigation transition profile captured no transition frames".into());
    }

    let expected_legs = [
        ("home-arcade", "forward"),
        ("home-arcade", "reverse"),
        ("home-consoles", "forward"),
        ("home-consoles", "reverse"),
        ("consoles-system", "forward"),
        ("consoles-system", "reverse"),
    ];
    let mut legs = serde_json::Map::new();
    let mut all_perfect = true;
    for (edge, direction) in expected_legs {
        let selected = transition_frames
            .iter()
            .copied()
            .filter(|frame| {
                frame
                    .get("navigation_transition_edge")
                    .and_then(Value::as_str)
                    == Some(edge)
                    && frame
                        .get("navigation_transition_direction")
                        .and_then(Value::as_str)
                        == Some(direction)
            })
            .collect::<Vec<_>>();
        if selected.len() < 2 {
            return Err(format!("navigation transition profile missed {edge} {direction}").into());
        }
        let first_us = frame_u64(selected[0], "completion_monotonic_us");
        let last_us = frame_u64(selected[selected.len() - 1], "completion_monotonic_us");
        let elapsed_us = last_us.saturating_sub(first_us).max(1);
        let fps = (selected.len().saturating_sub(1) as f64 * 1_000_000.0) / elapsed_us as f64;
        let sequence_gaps = selected
            .windows(2)
            .filter(|pair| {
                (frame_u64(pair[0], "main_present_sequence") as u16).wrapping_add(1)
                    != frame_u64(pair[1], "main_present_sequence") as u16
            })
            .count();
        let latch_drop_delta = (frame_u64(selected[selected.len() - 1], "main_present_drop_count")
            as u16)
            .wrapping_sub(frame_u64(selected[0], "main_present_drop_count") as u16);
        let mut transition_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "navigation_transition_us"))
            .collect::<Vec<_>>();
        let mut overlay_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "navigation_transition_overlay_us"))
            .collect::<Vec<_>>();
        let mut work_us = selected
            .iter()
            .map(|frame| {
                frame_u64(frame, "prepare_us")
                    + frame_u64(frame, "render_us")
                    + frame_u64(frame, "custom_draw_us")
                    + frame_u64(frame, "present_us")
            })
            .collect::<Vec<_>>();
        let mut prepare_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "prepare_us"))
            .collect::<Vec<_>>();
        let mut slint_render_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "render_us"))
            .collect::<Vec<_>>();
        transition_us.sort_unstable();
        overlay_us.sort_unstable();
        work_us.sort_unstable();
        prepare_us.sort_unstable();
        slint_render_us.sort_unstable();
        let status_worker_overlap_frames = selected
            .iter()
            .filter(|frame| {
                frame.get("status_worker_active").and_then(Value::as_bool) == Some(true)
            })
            .count();
        let status_submission_frames = selected
            .iter()
            .filter(|frame| frame_u64(frame, "status_enqueue_us") > 0)
            .count();
        let snapshot_unlocked_frames = selected
            .iter()
            .filter(|frame| {
                frame
                    .get("navigation_snapshot_locked")
                    .and_then(Value::as_bool)
                    != Some(true)
            })
            .count();
        let locked_slint_render_call_frames = selected
            .iter()
            .filter(|frame| {
                frame
                    .get("navigation_snapshot_locked")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && frame
                        .get("navigation_slint_render_called")
                        .and_then(Value::as_bool)
                        == Some(true)
            })
            .count();
        let status_quiesce_timeout_frames = selected
            .iter()
            .filter(|frame| {
                frame
                    .get("navigation_status_quiesce_timeout")
                    .and_then(Value::as_bool)
                    == Some(true)
            })
            .count();
        let status_quiesce_wait_max_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "navigation_status_quiesce_wait_us"))
            .max()
            .unwrap_or(0);
        let slint_timer_dispatch_max_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "slint_timer_dispatch_us"))
            .max()
            .unwrap_or(0);
        let navigation_commit_max_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "navigation_commit_us"))
            .max()
            .unwrap_or(0);
        let bridge_sync_max_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "bridge_sync_us"))
            .max()
            .unwrap_or(0);
        let unattributed_prepare_max_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "unattributed_prepare_us"))
            .max()
            .unwrap_or(0);
        let process_cpu_us = selected
            .iter()
            .map(|frame| frame_u64(frame, "process_cpu_us"))
            .sum::<u64>();
        let process_cpu_pct_one_core = process_cpu_us as f64 * 100.0 / elapsed_us as f64;
        let frame_work_p99_us = percentile_99(&work_us);
        let frame_work_max_us = work_us.last().copied().unwrap_or(0);
        let perfect_60 = sequence_gaps == 0
            && latch_drop_delta == 0
            && fps >= 59.9
            && frame_work_p99_us < 12_000
            && frame_work_max_us < 16_667
            && status_worker_overlap_frames == 0
            && status_submission_frames == 0
            && snapshot_unlocked_frames == 0
            && locked_slint_render_call_frames == 0
            && status_quiesce_timeout_frames == 0;
        all_perfect &= perfect_60;
        legs.insert(
            format!("{edge}-{direction}"),
            json!({
                "frames": selected.len(),
                "fps": fps,
                "perfect_60": perfect_60,
                "sequence_gaps": sequence_gaps,
                "latch_drop_delta": latch_drop_delta,
                "process_cpu_pct_of_one_core": process_cpu_pct_one_core,
                "transition_p99_us": percentile_99(&transition_us),
                "transition_max_us": transition_us.last().copied().unwrap_or(0),
                "overlay_p99_us": percentile_99(&overlay_us),
                "overlay_max_us": overlay_us.last().copied().unwrap_or(0),
                "frame_work_p99_us": frame_work_p99_us,
                "frame_work_max_us": frame_work_max_us,
                "prepare_p99_us": percentile_99(&prepare_us),
                "prepare_max_us": prepare_us.last().copied().unwrap_or(0),
                "slint_timer_dispatch_max_us": slint_timer_dispatch_max_us,
                "navigation_commit_max_us": navigation_commit_max_us,
                "bridge_sync_max_us": bridge_sync_max_us,
                "unattributed_prepare_max_us": unattributed_prepare_max_us,
                "slint_render_p99_us": percentile_99(&slint_render_us),
                "slint_render_max_us": slint_render_us.last().copied().unwrap_or(0),
                "locked_slint_render_call_frames": locked_slint_render_call_frames,
                "status_worker_overlap_frames": status_worker_overlap_frames,
                "status_submission_frames": status_submission_frames,
                "status_quiesce_wait_max_us": status_quiesce_wait_max_us,
                "status_quiesce_timeout_frames": status_quiesce_timeout_frames,
                "snapshot_unlocked_frames": snapshot_unlocked_frames,
            }),
        );
    }
    let system_cpu_pct = telemetry
        .iter()
        .filter_map(|sample| {
            sample
                .pointer("/cpu/combined_busy_pct")
                .and_then(Value::as_f64)
        })
        .collect::<Vec<_>>();
    let system_cpu_average =
        system_cpu_pct.iter().sum::<f64>() / system_cpu_pct.len().max(1) as f64;
    let summary = json!({
        "schema": "mister-magik-navigation-transition-profile-v2",
        "scenario": "navigation-transitions",
        "script": "Home -> Arcade -> Home -> Consoles -> System -> Consoles -> Home",
        "scanline_kernel": "neon-batched-rows",
        "spring_path_violations": 0,
        "all_legs_perfect_60": all_perfect,
        "transition_frames": transition_frames.len(),
        "system_combined_busy_pct_average": system_cpu_average,
        "cpu_profile": cpu_profile,
        "legs": legs,
        "telemetry_file": "telemetry.jsonl",
        "flamegraph_file": "flamegraph.svg",
        "folded_stacks_file": "stacks.folded",
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    let mut report = String::from("# Navigation transition profile: NEON batched rows\n\n");
    writeln!(report, "Perfect 60 FPS on every leg: **{}**\n", all_perfect)?;
    writeln!(
        report,
        "Average combined CPU busy: **{system_cpu_average:.1}%**\n"
    )?;
    writeln!(
        report,
        "CPU samples: **{} hits across {} unique stacks at {} Hz**\n",
        summary["cpu_profile"]["sample_hits"].as_i64().unwrap_or(0),
        summary["cpu_profile"]["sample_stacks"]
            .as_u64()
            .unwrap_or(0),
        summary["cpu_profile"]["hz"].as_i64().unwrap_or(0),
    )?;
    writeln!(
        report,
        "[Flamegraph](flamegraph.svg) · [Folded stacks](stacks.folded)\n"
    )?;
    writeln!(
        report,
        "| Leg | FPS | CPU (one core) | Work p99 | Prepare p99 | Transition p99 | Overlay p99 | Slint calls | Status overlap | Lock misses | Gaps | Drops |"
    )?;
    writeln!(
        report,
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;
    for (name, leg) in summary["legs"].as_object().into_iter().flatten() {
        writeln!(
            report,
            "| {name} | {:.2} | {:.1}% | {} us | {} us | {} us | {} us | {} | {} | {} | {} | {} |",
            leg["fps"].as_f64().unwrap_or(0.0),
            leg["process_cpu_pct_of_one_core"].as_f64().unwrap_or(0.0),
            leg["frame_work_p99_us"].as_u64().unwrap_or(0),
            leg["prepare_p99_us"].as_u64().unwrap_or(0),
            leg["transition_p99_us"].as_u64().unwrap_or(0),
            leg["overlay_p99_us"].as_u64().unwrap_or(0),
            leg["locked_slint_render_call_frames"].as_u64().unwrap_or(0),
            leg["status_worker_overlap_frames"].as_u64().unwrap_or(0),
            leg["snapshot_unlocked_frames"].as_u64().unwrap_or(0),
            leg["sequence_gaps"].as_u64().unwrap_or(0),
            leg["latch_drop_delta"].as_u64().unwrap_or(0),
        )?;
    }
    fs::write(output_dir.join("report.md"), report)?;
    Ok(summary)
}

fn catalog_lifecycle_evidence_command() -> String {
    format!(
        "set -eu; root={root}; find \"$root/diagnostics\" -maxdepth 1 -type f -print -exec sed -n '1,240p' {{}} \\; 2>/dev/null || true; {affinity}",
        root = sh(CATALOG_LIFECYCLE_REMOTE_DIR),
        affinity = catalog_lifecycle_affinity_command(),
    )
}

fn catalog_lifecycle_affinity_command() -> String {
    "pid=$(pidof mister-magik-fb | awk '{print $1}'); for task in /proc/$pid/task/*; do awk '/^(Name|Pid|Tgid|Cpus_allowed_list):/{print}' \"$task/status\"; awk '{print \"Nice:\\t\" $19}' \"$task/stat\"; done".to_string()
}

fn catalog_lifecycle_cleanup_command() -> String {
    format!(
        "set -eu; rm -rf {root}; test ! -e {root}",
        root = sh(CATALOG_LIFECYCLE_REMOTE_DIR),
    )
}

fn combine_catalog_lifecycle_restore(cleanup: Result<()>, resume: Result<()>) -> Result<()> {
    match (cleanup, resume) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(cleanup), Err(resume)) => {
            Err(format!("{cleanup}; launcher resume failed: {resume}").into())
        }
    }
}

fn parse_catalog_lifecycle_inspect(output: &str) -> Result<Value> {
    let mut valid = false;
    let mut generation = None;
    let mut total_games = None;
    let mut systems = Vec::new();
    for line in output.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("catalog_v3_summary_tsv") => {
                let values = fields
                    .filter_map(|field| field.split_once('='))
                    .collect::<std::collections::BTreeMap<_, _>>();
                valid = values.get("valid").copied() == Some("1");
                generation = values
                    .get("generation")
                    .and_then(|value| value.parse::<u64>().ok());
                total_games = values
                    .get("total_games")
                    .and_then(|value| value.parse::<u64>().ok());
            }
            Some("catalog_v3_system_tsv") => {
                let values = fields
                    .filter_map(|field| field.split_once('='))
                    .collect::<std::collections::BTreeMap<_, _>>();
                let system = values
                    .get("system")
                    .ok_or("catalog lifecycle system row has no system")?;
                let games = values
                    .get("registry_games")
                    .ok_or("catalog lifecycle system row has no game count")?
                    .parse::<u64>()?;
                systems.push(json!({
                    "system": system,
                    "games": games,
                    "role": values.get("role").copied().unwrap_or("unknown"),
                }));
            }
            _ => {}
        }
    }
    if !valid {
        return Err("catalog lifecycle inspection did not report valid=1".into());
    }
    Ok(json!({
        "valid": true,
        "generation": generation.ok_or("catalog lifecycle inspection has no generation")?,
        "total_games": total_games.ok_or("catalog lifecycle inspection has no total game count")?,
        "systems": systems,
    }))
}

fn catalog_lifecycle_report(summary: &Value) -> Result<String> {
    let elapsed_ms = summary
        .get("elapsed_ms")
        .and_then(Value::as_u64)
        .ok_or("catalog lifecycle summary has no elapsed time")?;
    let total_games = summary
        .pointer("/catalog/total_games")
        .and_then(Value::as_u64)
        .ok_or("catalog lifecycle summary has no total game count")?;
    let systems = summary
        .pointer("/catalog/systems")
        .and_then(Value::as_array)
        .ok_or("catalog lifecycle summary has no systems")?;
    let mut report = format!(
        "# Catalog Lifecycle Benchmark\n\n- Result: passed\n- Elapsed: {elapsed_ms} ms\n- Total games: {total_games}\n- Systems: {}\n\n## Systems\n\n",
        systems.len()
    );
    for system in systems {
        report.push_str(&format!(
            "- {}: {} games\n",
            system
                .get("system")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            system.get("games").and_then(Value::as_u64).unwrap_or(0)
        ));
    }
    Ok(report)
}

fn profile_installed_particles(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    run: ParticleBenchmarkRun,
) -> Result<String> {
    const DISPLAY_MODE: &str = "hdmi-1920x1080p60";
    let benchmark_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == DISPLAY_MODE)
        .copied()
        .ok_or("particle benchmark display mode is unavailable")?;
    let session = connect_with(&config.connection, 10)?;
    let capability = exec_checked_output(
        &session,
        "installed particle benchmark capability",
        "/media/fat/mister-magik-dev/mister-magik-fb benchmark-capabilities",
    )?;
    let capability = last_json_line(&capability.stdout)
        .ok_or("installed benchmark capability output contains no JSON report")?;
    if capability
        .get("particle-capacity-v1")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("installed app does not support particle-capacity-v1".into());
    }
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing")?;
    let boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable")?
        .trim()
        .to_string();
    let original_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    let original_reply = exec_checked_output(
        &session,
        "query original particle benchmark display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("particle benchmark cannot start during a display transaction".into());
    }
    let original_mode_spec = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == original_mode)
        .copied()
        .ok_or_else(|| format!("particle benchmark cannot restore unknown mode {original_mode}"))?;
    fs::create_dir_all(output_dir)?;
    drop(session);
    let _signal_guard = ScreensaverProfileSignalGuard::install();

    let run_result = (|| -> Result<Value> {
        apply_confirmed_display_mode(config, benchmark_mode, "particle benchmark")?;
        let session = connect_with(&config.connection, 10)?;
        validate_particle_display_geometry(&session)?;
        drop(session);
        match run {
            ParticleBenchmarkRun::Complete => {
                let capacity = profile_particle_preset(config, output_dir, "capacity")?;
                let visual = profile_particle_preset(config, output_dir, "visual")?;
                let visual_count = visual
                    .get("confirmed_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let captures = if visual_count > 0 {
                    capture_particle_phases(config, output_dir, visual_count)?
                } else {
                    Value::Null
                };
                Ok(json!({
                    "capacity": capacity,
                    "visual": visual,
                    "captures": captures,
                }))
            }
            ParticleBenchmarkRun::Capacity => Ok(json!({
                "capacity": profile_particle_preset(config, output_dir, "capacity")?,
            })),
            ParticleBenchmarkRun::Demo40k => Ok(json!({
                "demo": run_particle_trial(
                    config,
                    output_dir,
                    "visual",
                    PARTICLE_DEMO_40K_COUNT,
                    PARTICLE_DEMO_40K_DURATION_SECS,
                    "demo-40k",
                )?,
            })),
            ParticleBenchmarkRun::Step => Ok(json!({
                "step": run_particle_trial(
                    config,
                    output_dir,
                    "capacity",
                    PARTICLE_STEP_COUNT,
                    PARTICLE_STEP_DURATION_SECS,
                    "step",
                )?,
            })),
            ParticleBenchmarkRun::Showcase(demo_number) => {
                let (label, count) = particle_showcase_demo(demo_number)?;
                let demo =
                    run_particle_showcase_trial(config, output_dir, demo_number, label, count)?;
                let captures =
                    capture_particle_showcase_frame(config, output_dir, demo_number, label, count)?;
                Ok(json!({
                    "demo": demo,
                    "captures": captures,
                }))
            }
        }
    })();

    let launcher_cleanup = restore_installed_screensaver_profile(config);
    let display_restore =
        apply_confirmed_display_mode(config, original_mode_spec, "particle benchmark restoration");
    let final_verification = display_restore.and_then(|()| {
        verify_particle_benchmark_restoration(
            config,
            &original_mode,
            &original_ini,
            &boot_id,
            &manifest,
        )
    });
    let cleanup_result = combine_benchmark_cleanup(launcher_cleanup, final_verification);
    let results = match (run_result, cleanup_result) {
        (Ok(results), Ok(())) => results,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("particle benchmark cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(
                format!("{run_error}; particle benchmark cleanup failed: {cleanup_error}").into(),
            );
        }
    };
    let schema = match run {
        ParticleBenchmarkRun::Complete => "mister-magik-particle-benchmark-v1",
        ParticleBenchmarkRun::Capacity => "mister-magik-particle-capacity-benchmark-v1",
        ParticleBenchmarkRun::Demo40k => "mister-magik-particle-demo-40k-v1",
        ParticleBenchmarkRun::Step => "mister-magik-particle-step-v1",
        ParticleBenchmarkRun::Showcase(_) => "mister-magik-particle-showcase-v1",
    };
    let summary = json!({
        "schema": schema,
        "display": {
            "benchmark_mode": benchmark_mode.id,
            "framebuffer": "960x540",
            "bits_per_pixel": 16,
            "original_mode": original_mode,
            "restored": true,
        },
        "search": {
            "start_count": PARTICLE_COUNT_STEP,
            "maximum_count": PARTICLE_COUNT_MAX,
            "refinement_step": PARTICLE_COUNT_STEP,
            "trial_seconds": PARTICLE_SEARCH_TRIAL_SECS,
            "confirmation_seconds": PARTICLE_CONFIRMATION_SECS,
            "post_reserve_us": PARTICLE_POST_RESERVE_US,
        },
        "presets": {
            "capacity": results.get("capacity").cloned().unwrap_or(Value::Null),
            "visual": results.get("visual").cloned().unwrap_or(Value::Null),
        },
        "demo": results.get("demo").cloned().unwrap_or(Value::Null),
        "step": results.get("step").cloned().unwrap_or(Value::Null),
        "showcase_demo": match run {
            ParticleBenchmarkRun::Showcase(demo) => Value::from(demo),
            _ => Value::Null,
        },
        "captures": results.get("captures").cloned().unwrap_or(Value::Null),
        "boot_id": boot_id,
        "manifest": parse_manifest_evidence(&manifest),
        "output_dir": output_dir,
    });
    persist_and_qualify_particle_benchmark(output_dir, &summary, run)
}

fn profile_installed_particle_cpu(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    const DISPLAY_MODE: &str = "hdmi-1920x1080p60";
    let benchmark_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == DISPLAY_MODE)
        .copied()
        .ok_or("particle CPU profile display mode is unavailable")?;
    let session = connect_with(&config.connection, 10)?;
    let capability = exec_checked_output(
        &session,
        "installed particle CPU profile capability",
        "/media/fat/mister-magik-dev/mister-magik-fb benchmark-capabilities",
    )?;
    let capability = last_json_line(&capability.stdout)
        .ok_or("installed benchmark capability output contains no JSON report")?;
    for capability_name in ["particle-capacity-v1", "screensaver-pprof-v1"] {
        if capability.get(capability_name).and_then(Value::as_bool) != Some(true) {
            return Err(format!("installed app does not support {capability_name}").into());
        }
    }
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing")?;
    let boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable")?
        .trim()
        .to_string();
    let original_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    let original_reply = exec_checked_output(
        &session,
        "query original particle CPU profile display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("particle CPU profile cannot start during a display transaction".into());
    }
    let original_mode_spec = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == original_mode)
        .copied()
        .ok_or_else(|| {
            format!("particle CPU profile cannot restore unknown mode {original_mode}")
        })?;
    fs::create_dir_all(output_dir)?;
    drop(session);
    let _signal_guard = ScreensaverProfileSignalGuard::install();

    let run_result = (|| -> Result<Value> {
        apply_confirmed_display_mode(config, benchmark_mode, "particle CPU profile")?;
        let session = connect_with(&config.connection, 10)?;
        validate_particle_display_geometry(&session)?;
        drop(session);
        let capacity = profile_particle_cpu_preset(
            config,
            output_dir,
            "capacity",
            PARTICLE_CPU_PROFILE_CAPACITY_COUNT,
        )?;
        let visual = profile_particle_cpu_preset(
            config,
            output_dir,
            "visual",
            PARTICLE_CPU_PROFILE_VISUAL_COUNT,
        )?;
        Ok(json!({
            "capacity": capacity,
            "visual": visual,
        }))
    })();

    let launcher_cleanup = restore_installed_screensaver_profile(config);
    let display_restore = apply_confirmed_display_mode(
        config,
        original_mode_spec,
        "particle CPU profile restoration",
    );
    let final_verification = display_restore.and_then(|()| {
        verify_particle_benchmark_restoration(
            config,
            &original_mode,
            &original_ini,
            &boot_id,
            &manifest,
        )
    });
    let cleanup_result = combine_benchmark_cleanup(launcher_cleanup, final_verification);
    let presets = match (run_result, cleanup_result) {
        (Ok(results), Ok(())) => results,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("particle CPU profile cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(format!(
                "{run_error}; particle CPU profile cleanup failed: {cleanup_error}"
            )
            .into());
        }
    };
    let summary = json!({
        "schema": "mister-magik-particle-cpu-profile-v1",
        "display": {
            "benchmark_mode": benchmark_mode.id,
            "framebuffer": "960x540",
            "bits_per_pixel": 16,
            "original_mode": original_mode,
            "restored": true,
        },
        "duration_secs": PARTICLE_CPU_PROFILE_DURATION_SECS,
        "sampling_hz": 99,
        "presets": presets,
        "boot_id": boot_id,
        "manifest": parse_manifest_evidence(&manifest),
        "output_dir": output_dir,
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn profile_installed_particle_showcase_cpu(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
) -> Result<String> {
    const DISPLAY_MODE: &str = "hdmi-1920x1080p60";
    let (label, count) = particle_showcase_demo(demo_number)?;
    let benchmark_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == DISPLAY_MODE)
        .copied()
        .ok_or("particle showcase CPU profile display mode is unavailable")?;
    let session = connect_with(&config.connection, 10)?;
    let capability = exec_checked_output(
        &session,
        "installed particle showcase CPU profile capability",
        "/media/fat/mister-magik-dev/mister-magik-fb benchmark-capabilities",
    )?;
    let capability = last_json_line(&capability.stdout)
        .ok_or("installed benchmark capability output contains no JSON report")?;
    for capability_name in ["particle-showcase-v1", "screensaver-pprof-v1"] {
        if capability.get(capability_name).and_then(Value::as_bool) != Some(true) {
            return Err(format!("installed app does not support {capability_name}").into());
        }
    }
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing")?;
    let boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable")?
        .trim()
        .to_string();
    let original_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    let original_reply = exec_checked_output(
        &session,
        "query original particle showcase CPU profile display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err(
            "particle showcase CPU profile cannot start during a display transaction".into(),
        );
    }
    let original_mode_spec = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == original_mode)
        .copied()
        .ok_or_else(|| {
            format!("particle showcase CPU profile cannot restore unknown mode {original_mode}")
        })?;
    fs::create_dir_all(output_dir)?;
    drop(session);
    let _signal_guard = ScreensaverProfileSignalGuard::install();

    let run_result = (|| -> Result<Value> {
        apply_confirmed_display_mode(config, benchmark_mode, "particle showcase CPU profile")?;
        let session = connect_with(&config.connection, 10)?;
        validate_particle_display_geometry(&session)?;
        drop(session);
        profile_particle_showcase_cpu_demo(config, output_dir, demo_number, label, count)
    })();

    let launcher_cleanup = restore_installed_screensaver_profile(config);
    let display_restore = apply_confirmed_display_mode(
        config,
        original_mode_spec,
        "particle showcase CPU profile restoration",
    );
    let final_verification = display_restore.and_then(|()| {
        verify_particle_benchmark_restoration(
            config,
            &original_mode,
            &original_ini,
            &boot_id,
            &manifest,
        )
    });
    let cleanup_result = combine_benchmark_cleanup(launcher_cleanup, final_verification);
    let profile = match (run_result, cleanup_result) {
        (Ok(profile), Ok(())) => profile,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("particle showcase CPU profile cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(format!(
                "{run_error}; particle showcase CPU profile cleanup failed: {cleanup_error}"
            )
            .into());
        }
    };
    let summary = json!({
        "schema": "mister-magik-particle-showcase-cpu-profile-v1",
        "display": {
            "benchmark_mode": benchmark_mode.id,
            "framebuffer": "960x540",
            "bits_per_pixel": 16,
            "original_mode": original_mode,
            "restored": true,
        },
        "duration_secs": PARTICLE_CPU_PROFILE_DURATION_SECS,
        "sampling_hz": 99,
        "demo_number": demo_number,
        "demo": profile,
        "boot_id": boot_id,
        "manifest": parse_manifest_evidence(&manifest),
        "output_dir": output_dir,
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn profile_particle_showcase_cpu_demo(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
    label: &str,
    count: u64,
) -> Result<Value> {
    let remote_svg =
        format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-showcase-{demo_number:02}.svg");
    let remote_folded =
        format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-showcase-{demo_number:02}.folded");
    let remote_complete =
        format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-showcase-{demo_number:02}.json");
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "reset particle showcase CPU profile artifacts",
        &format!(
            "set -eu; mkdir -p {0}; rm -f {1} {2} {3}",
            sh(SCREENSAVER_PROFILE_REMOTE_DIR),
            sh(&remote_svg),
            sh(&remote_folded),
            sh(&remote_complete)
        ),
    )?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-demos".into(),
                ),
                ("MISTER_PARTICLE_DEMO".into(), demo_number.to_string()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                ("MISTER_PARTICLE_HUD".into(), "off".into()),
                ("MISTER_PPROF".into(), "1".into()),
                ("MISTER_PPROF_TRIGGER".into(), "screensaver".into()),
                (
                    "MISTER_PPROF_DURATION_SECS".into(),
                    PARTICLE_CPU_PROFILE_DURATION_SECS.to_string(),
                ),
                ("MISTER_PPROF_HZ".into(), "99".into()),
                ("MISTER_PPROF_OUT".into(), remote_svg.clone()),
                ("MISTER_PPROF_FOLDED_OUT".into(), remote_folded.clone()),
                ("MISTER_PPROF_COMPLETE".into(), remote_complete.clone()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);

    let telemetry = agent_telemetry_until_screensaver_profile_complete(
        &config.agent,
        Duration::from_secs(PARTICLE_CPU_PROFILE_DURATION_SECS + 20),
    )?;
    let telemetry_file = format!("{demo_number:02}-{label}-profile-telemetry.jsonl");
    let timing = summarize_particle_trial_for_renderer(
        label,
        count,
        PARTICLE_CPU_PROFILE_DURATION_SECS,
        "profile",
        &telemetry_file,
        &telemetry,
        "particle-demos",
    );
    if timing.get("frames").and_then(Value::as_u64).unwrap_or(0) == 0 {
        return Err(format!("particle showcase profile did not attest demo {demo_number}").into());
    }
    let session = connect_with(&config.connection, 10)?;
    let metadata = remote_read(&session, &remote_complete)
        .ok_or("particle showcase profile completion metadata is missing")?;
    let metadata_value: Value = serde_json::from_str(metadata.trim())?;
    if metadata_value.get("state").and_then(Value::as_str) != Some("complete")
        || metadata_value
            .get("sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            <= 0
    {
        return Err(format!("particle showcase demo {demo_number} produced no CPU samples").into());
    }
    let svg = remote_read(&session, &remote_svg)
        .filter(|text| !text.is_empty())
        .ok_or("particle showcase profile SVG is missing")?;
    let folded = remote_read(&session, &remote_folded)
        .filter(|text| !text.is_empty())
        .ok_or("particle showcase folded stacks are missing")?;
    let svg_file = format!("{demo_number:02}-{label}.svg");
    let folded_file = format!("{demo_number:02}-{label}.folded");
    let profile_file = format!("{demo_number:02}-{label}-profile.json");
    fs::write(output_dir.join(&svg_file), svg)?;
    fs::write(output_dir.join(&folded_file), folded)?;
    fs::write(
        output_dir.join(&profile_file),
        format!("{}\n", serde_json::to_string_pretty(&metadata_value)?),
    )?;
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(
        output_dir.join(&telemetry_file),
        format!("{telemetry_text}\n"),
    )?;
    Ok(json!({
        "label": label,
        "count": count,
        "profile": metadata_value,
        "timing": timing,
        "artifacts": {
            "svg": svg_file,
            "folded": folded_file,
            "metadata": profile_file,
            "telemetry": telemetry_file,
        },
    }))
}

fn profile_particle_cpu_preset(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    preset: &str,
    count: u64,
) -> Result<Value> {
    let remote_svg = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-{preset}.svg");
    let remote_folded = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-{preset}.folded");
    let remote_complete = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/particle-{preset}.json");
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "reset particle CPU profile artifacts",
        &format!(
            "set -eu; mkdir -p {0}; rm -f {1} {2} {3}",
            sh(SCREENSAVER_PROFILE_REMOTE_DIR),
            sh(&remote_svg),
            sh(&remote_folded),
            sh(&remote_complete)
        ),
    )?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-magik".into(),
                ),
                ("MISTER_PARTICLE_COUNT".into(), count.to_string()),
                ("MISTER_PARTICLE_PRESET".into(), preset.into()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                ("MISTER_PPROF".into(), "1".into()),
                ("MISTER_PPROF_TRIGGER".into(), "screensaver".into()),
                (
                    "MISTER_PPROF_DURATION_SECS".into(),
                    PARTICLE_CPU_PROFILE_DURATION_SECS.to_string(),
                ),
                ("MISTER_PPROF_HZ".into(), "99".into()),
                ("MISTER_PPROF_OUT".into(), remote_svg.clone()),
                ("MISTER_PPROF_FOLDED_OUT".into(), remote_folded.clone()),
                ("MISTER_PPROF_COMPLETE".into(), remote_complete.clone()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);

    let telemetry = agent_telemetry_until_screensaver_profile_complete(
        &config.agent,
        Duration::from_secs(PARTICLE_CPU_PROFILE_DURATION_SECS + 20),
    )?;
    let telemetry_file = format!("{preset}-telemetry.jsonl");
    let timing = summarize_particle_trial(
        preset,
        count,
        PARTICLE_CPU_PROFILE_DURATION_SECS,
        "profile",
        &telemetry_file,
        &telemetry,
    );
    if timing.get("frames").and_then(Value::as_u64).unwrap_or(0) == 0 {
        return Err(format!("particle CPU profile did not attest {preset} count {count}").into());
    }
    let session = connect_with(&config.connection, 10)?;
    let metadata = remote_read(&session, &remote_complete)
        .ok_or("particle CPU profile completion metadata is missing")?;
    let metadata_value: Value = serde_json::from_str(metadata.trim())?;
    if metadata_value.get("state").and_then(Value::as_str) != Some("complete")
        || metadata_value
            .get("sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            <= 0
    {
        return Err(format!("particle CPU profile for {preset} produced no CPU samples").into());
    }
    let svg = remote_read(&session, &remote_svg)
        .filter(|text| !text.is_empty())
        .ok_or("particle CPU profile SVG is missing")?;
    let folded = remote_read(&session, &remote_folded)
        .filter(|text| !text.is_empty())
        .ok_or("particle CPU profile folded stacks are missing")?;
    let svg_file = format!("{preset}.svg");
    let folded_file = format!("{preset}.folded");
    let profile_file = format!("{preset}-profile.json");
    fs::write(output_dir.join(&svg_file), svg)?;
    fs::write(output_dir.join(&folded_file), folded)?;
    fs::write(
        output_dir.join(&profile_file),
        format!("{}\n", serde_json::to_string_pretty(&metadata_value)?),
    )?;
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(
        output_dir.join(&telemetry_file),
        format!("{telemetry_text}\n"),
    )?;
    Ok(json!({
        "preset": preset,
        "count": count,
        "profile": metadata_value,
        "timing": timing,
        "artifacts": {
            "svg": svg_file,
            "folded": folded_file,
            "metadata": profile_file,
            "telemetry": telemetry_file,
        },
    }))
}

fn profile_particle_preset(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    preset: &str,
) -> Result<Value> {
    let mut trials = Vec::new();
    let mut last_pass = 0u64;
    let mut first_fail = None;
    let mut count = PARTICLE_COUNT_STEP;
    while count <= PARTICLE_COUNT_MAX {
        let trial = run_particle_trial(
            config,
            output_dir,
            preset,
            count,
            PARTICLE_SEARCH_TRIAL_SECS,
            "search",
        )?;
        let qualified = trial.get("qualified").and_then(Value::as_bool) == Some(true);
        trials.push(trial);
        if qualified {
            last_pass = count;
            if count == PARTICLE_COUNT_MAX {
                break;
            }
            count = count.saturating_mul(2).min(PARTICLE_COUNT_MAX);
        } else {
            first_fail = Some(count);
            break;
        }
    }
    if let Some(mut upper) = first_fail {
        while let Some(middle) = particle_refinement_count(last_pass, upper) {
            let trial = run_particle_trial(
                config,
                output_dir,
                preset,
                middle,
                PARTICLE_SEARCH_TRIAL_SECS,
                "refine",
            )?;
            let qualified = trial.get("qualified").and_then(Value::as_bool) == Some(true);
            trials.push(trial);
            if qualified {
                last_pass = middle;
            } else {
                upper = middle;
                first_fail = Some(middle);
            }
        }
    }
    let mut confirmation_count = last_pass;
    let mut confirmation_attempts = Vec::new();
    while confirmation_count > 0 {
        let attempt = run_particle_trial(
            config,
            output_dir,
            preset,
            confirmation_count,
            PARTICLE_CONFIRMATION_SECS,
            "confirm",
        )?;
        let qualified = attempt.get("qualified").and_then(Value::as_bool) == Some(true);
        confirmation_attempts.push(attempt);
        if qualified {
            break;
        }
        first_fail =
            Some(first_fail.map_or(confirmation_count, |upper| upper.min(confirmation_count)));
        confirmation_count = particle_confirmation_backoff(confirmation_count).unwrap_or_default();
    }
    let confirmation = confirmation_attempts.last().cloned().unwrap_or_else(|| {
        json!({
            "qualified": false,
            "failures": [{"kind": "minimum-count-failed"}],
        })
    });
    let confirmed = confirmation.get("qualified").and_then(Value::as_bool) == Some(true);
    let memory = confirmation.get("memory").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "preset": preset,
        "confirmed_count": if confirmed { confirmation_count } else { 0 },
        "first_failing_count": first_fail,
        "upper_bound_reached": confirmed
            && confirmation_count == PARTICLE_COUNT_MAX
            && first_fail.is_none(),
        "bytes_per_particle": memory
            .get("simulation_bytes_per_particle")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "memory": memory,
        "trials": trials,
        "confirmation_attempts": confirmation_attempts,
        "confirmation": confirmation,
    }))
}

fn particle_refinement_count(last_pass: u64, first_fail: u64) -> Option<u64> {
    let distance = first_fail.saturating_sub(last_pass);
    if distance <= PARTICLE_COUNT_STEP {
        return None;
    }
    let slots = distance / PARTICLE_COUNT_STEP;
    Some(last_pass + (slots / 2).max(1) * PARTICLE_COUNT_STEP)
}

fn particle_confirmation_backoff(count: u64) -> Option<u64> {
    count
        .checked_sub(PARTICLE_COUNT_STEP)
        .filter(|next| *next > 0)
}

fn run_particle_trial(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    preset: &str,
    count: u64,
    duration_secs: u64,
    kind: &str,
) -> Result<Value> {
    let session = connect_with(&config.connection, 10)?;
    let mut env_vars = vec![
        ("MISTER_CATALOG_REFRESH".into(), "off".into()),
        ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
        (
            "MISTER_SCREENSAVER_RENDERER".into(),
            "particle-magik".into(),
        ),
        ("MISTER_PARTICLE_COUNT".into(), count.to_string()),
        ("MISTER_PARTICLE_PRESET".into(), preset.into()),
        ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
    ];
    if kind == "step" {
        env_vars.push(("MISTER_PARTICLE_PMU".into(), "1".into()));
    }
    if kind == "demo-40k" {
        env_vars.push(("MISTER_PARTICLE_PROJECTION_VALIDATE".into(), "1".into()));
    }
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars,
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);
    let telemetry = agent_telemetry_for_particle_trial(
        &config.agent,
        preset,
        count,
        Duration::from_secs(duration_secs),
        Duration::from_secs(10),
    )?;
    let filename = format!("{preset}-{count}-{kind}-telemetry.jsonl");
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(output_dir.join(&filename), format!("{telemetry_text}\n"))?;
    Ok(summarize_particle_trial(
        preset,
        count,
        duration_secs,
        kind,
        &filename,
        &telemetry,
    ))
}

fn run_particle_showcase_trial(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
    label: &str,
    count: u64,
) -> Result<Value> {
    let session = connect_with(&config.connection, 10)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-demos".into(),
                ),
                ("MISTER_PARTICLE_DEMO".into(), demo_number.to_string()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                ("MISTER_PARTICLE_HUD".into(), "off".into()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);
    let telemetry = agent_telemetry_for_particle_renderer_trial(
        &config.agent,
        "particle-demos",
        label,
        count,
        Duration::from_secs(PARTICLE_SHOWCASE_DURATION_SECS),
        Duration::from_secs(10),
    )?;
    let filename = format!("{demo_number:02}-{label}-telemetry.jsonl");
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(output_dir.join(&filename), format!("{telemetry_text}\n"))?;
    Ok(summarize_particle_trial_for_renderer(
        label,
        count,
        PARTICLE_SHOWCASE_DURATION_SECS,
        "showcase",
        &filename,
        &telemetry,
        "particle-demos",
    ))
}

fn launch_particle_showcase_interactive(config: &NativeDeviceConfig) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    validate_particle_display_geometry(&session)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-demos".into(),
                ),
                ("MISTER_PARTICLE_DEMO".into(), "1".into()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    Ok("particle showcase is running; left/right changes demos and any other input exits".into())
}

fn capture_particle_showcase_frame(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
    label: &str,
    count: u64,
) -> Result<Value> {
    let session = connect_with(&config.connection, 10)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-demos".into(),
                ),
                ("MISTER_PARTICLE_DEMO".into(), demo_number.to_string()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                ("MISTER_PARTICLE_HUD".into(), "off".into()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);
    let telemetry = agent_telemetry_for_duration(&config.agent, Duration::from_secs(12))?;
    let latest = telemetry
        .iter()
        .filter_map(|sample| {
            sample
                .pointer("/launcher/frame_budget/recent_frames")
                .and_then(Value::as_array)
        })
        .flatten()
        .filter(|frame| {
            frame.get("screensaver_active").and_then(Value::as_bool) == Some(true)
                && frame.get("screensaver_renderer").and_then(Value::as_str)
                    == Some("particle-demos")
                && frame.get("particle_preset").and_then(Value::as_str) == Some(label)
                && frame.get("particle_count").and_then(Value::as_u64) == Some(count)
        })
        .max_by_key(|frame| frame.get("frame").and_then(Value::as_u64).unwrap_or(0))
        .ok_or_else(|| {
            format!("particle showcase capture did not observe demo {demo_number:02} {label}")
        })?;
    let beat = latest
        .get("particle_phase")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let capture = request_framebuffer_png_at_when_latched(&config.agent, Duration::from_secs(3))?;
    validate_visible_launcher_capture(&capture)?;
    let filename = format!("{demo_number:02}-{label}-{beat}.png");
    let path = output_dir.join(filename);
    fs::write(&path, &capture.png)?;
    Ok(json!({
        "representative": path,
        "beat": beat,
        "count": count,
        "source": capture_source_label(&capture.result)?,
    }))
}

fn capture_installed_firework_visual(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
    label: &str,
    time_ms: u64,
) -> Result<String> {
    if !(1..=12).contains(&demo_number) {
        return Err(format!(
            "firework visual capture demo must be in 1..=12, received {demo_number}"
        )
        .into());
    }
    let (expected_label, _) = particle_showcase_demo(demo_number)?;
    if label != expected_label {
        return Err(format!(
            "firework visual demo {demo_number} is {expected_label:?}, received {label:?}"
        )
        .into());
    }
    if time_ms > 10_000 {
        return Err(
            format!("firework visual time must be at most 10000 ms, received {time_ms}").into(),
        );
    }
    fs::create_dir_all(output_dir)?;
    let run_result = (|| -> Result<Value> {
        let session = connect_with(&config.connection, 10)?;
        validate_particle_display_geometry(&session)?;
        restart_launcher_with_one_shot_env(
            &session,
            LauncherRestartOptions {
                env_vars: vec![
                    ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                    ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                    (
                        "MISTER_SCREENSAVER_RENDERER".into(),
                        "particle-demos".into(),
                    ),
                    ("MISTER_PARTICLE_DEMO".into(), demo_number.to_string()),
                    ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                    ("MISTER_FIREWORK_TIME_MS".into(), time_ms.to_string()),
                    ("MISTER_PARTICLE_HUD".into(), "off".into()),
                ],
                timeout_secs: 45,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                ..LauncherRestartOptions::default()
            },
        )?;
        drop(session);
        let telemetry = agent_telemetry_for_duration(&config.agent, Duration::from_secs(5))?;
        let frames = telemetry
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/frame_budget/recent_frames")
                    .and_then(Value::as_array)
            })
            .flatten()
            .filter(|frame| frame.get("screensaver_active").and_then(Value::as_bool) == Some(true))
            .collect::<Vec<_>>();
        if frames.iter().any(|frame| {
            frame.get("screensaver_renderer").and_then(Value::as_str) == Some("particle-magik")
        }) {
            return Err(
                "firework visual capture observed the forbidden particle-magik renderer".into(),
            );
        }
        let first_active_renderer = first_declared_screensaver_renderer(&frames)
            .ok_or("firework visual capture observed no active screensaver frame")?;
        if first_active_renderer != "particle-demos" {
            return Err(format!(
                "firework visual capture started with {first_active_renderer:?}, expected particle-demos"
            )
            .into());
        }
        if !frames.iter().any(|frame| {
            frame.get("screensaver_renderer").and_then(Value::as_str) == Some("particle-demos")
                && frame.get("particle_preset").and_then(Value::as_str) == Some(label)
        }) {
            return Err(format!(
                "firework visual capture did not observe demo {demo_number:02} {label}"
            )
            .into());
        }
        let capture = request_framebuffer_png_at(&config.agent)?;
        validate_visible_launcher_capture(&capture)?;
        let filename = format!("{demo_number:02}-{label}-{time_ms}ms.png");
        let path = output_dir.join(&filename);
        fs::write(&path, &capture.png)?;
        Ok(json!({
            "schema": "mister-magik-firework-visual-v2",
            "demo_number": demo_number,
            "firework": label,
            "time_ms": time_ms,
            "seed": 827141709451_u64,
            "first_active_renderer": first_active_renderer,
            "particle_magik_observed": false,
            "capture": filename,
            "source": capture_source_label(&capture.result)?,
        }))
    })();
    let cleanup_result = restore_installed_screensaver_profile(config);
    let summary = match (run_result, cleanup_result) {
        (Ok(summary), Ok(())) => summary,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("firework visual cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(
                format!("{run_error}; firework visual cleanup failed: {cleanup_error}").into(),
            );
        }
    };
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn capture_installed_particle_technique(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    demo_number: u8,
    label: &str,
    hero_secs: u64,
) -> Result<String> {
    if !(24..=31).contains(&demo_number) {
        return Err(format!(
            "particle technique capture demo must be in 24..=31, received {demo_number}"
        )
        .into());
    }
    let (expected_label, count) = particle_showcase_demo(demo_number)?;
    if label != expected_label {
        return Err(format!(
            "particle technique demo {demo_number} is {expected_label:?}, received {label:?}"
        )
        .into());
    }
    if !(1..=30).contains(&hero_secs) {
        return Err(format!(
            "particle technique hero time must be in 1..=30 seconds, received {hero_secs}"
        )
        .into());
    }
    fs::create_dir_all(output_dir)?;
    let run_result = (|| -> Result<Value> {
        let session = connect_with(&config.connection, 10)?;
        validate_particle_display_geometry(&session)?;
        restart_launcher_with_one_shot_env(
            &session,
            LauncherRestartOptions {
                env_vars: vec![
                    ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                    ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                    (
                        "MISTER_SCREENSAVER_RENDERER".into(),
                        "particle-demos".into(),
                    ),
                    ("MISTER_PARTICLE_DEMO".into(), demo_number.to_string()),
                    ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
                    ("MISTER_PARTICLE_HUD".into(), "off".into()),
                ],
                timeout_secs: 45,
                remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
                ..LauncherRestartOptions::default()
            },
        )?;
        drop(session);
        let telemetry =
            agent_telemetry_for_duration(&config.agent, Duration::from_secs(hero_secs))?;
        let latest = telemetry
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/frame_budget/recent_frames")
                    .and_then(Value::as_array)
            })
            .flatten()
            .filter(|frame| {
                frame.get("screensaver_active").and_then(Value::as_bool) == Some(true)
                    && frame.get("screensaver_renderer").and_then(Value::as_str)
                        == Some("particle-demos")
                    && frame.get("particle_preset").and_then(Value::as_str) == Some(label)
                    && frame.get("particle_count").and_then(Value::as_u64) == Some(count)
            })
            .max_by_key(|frame| frame.get("frame").and_then(Value::as_u64).unwrap_or(0))
            .ok_or_else(|| {
                format!("particle technique capture did not observe demo {demo_number:02} {label}")
            })?;
        let beat = latest
            .get("particle_phase")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let capture =
            request_framebuffer_png_at_when_latched(&config.agent, Duration::from_secs(3))?;
        validate_visible_launcher_capture(&capture)?;
        let filename = format!("{demo_number:02}-{label}-{beat}.png");
        fs::write(output_dir.join(&filename), &capture.png)?;
        Ok(json!({
            "schema": "mister-magik-particle-technique-capture-v1",
            "demo_number": demo_number,
            "technique": label,
            "hero_secs": hero_secs,
            "beat": beat,
            "count": count,
            "seed": 827141709451_u64,
            "geometry": "960x540",
            "pixel_format": "RGB565",
            "capture": filename,
            "source": capture_source_label(&capture.result)?,
        }))
    })();
    let cleanup_result = restore_installed_screensaver_profile(config);
    let summary = match (run_result, cleanup_result) {
        (Ok(summary), Ok(())) => summary,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("particle technique capture cleanup failed: {error}").into());
        }
        (Err(run_error), Err(cleanup_error)) => {
            return Err(format!(
                "{run_error}; particle technique capture cleanup failed: {cleanup_error}"
            )
            .into());
        }
    };
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn first_declared_screensaver_renderer<'a>(frames: &'a [&Value]) -> Option<&'a str> {
    frames.iter().find_map(|frame| {
        frame
            .get("screensaver_renderer")
            .and_then(Value::as_str)
            .filter(|renderer| !renderer.is_empty())
    })
}

fn summarize_particle_trial(
    preset: &str,
    count: u64,
    duration_secs: u64,
    kind: &str,
    telemetry_file: &str,
    telemetry: &[Value],
) -> Value {
    summarize_particle_trial_for_renderer(
        preset,
        count,
        duration_secs,
        kind,
        telemetry_file,
        telemetry,
        "particle-magik",
    )
}

fn summarize_particle_trial_for_renderer(
    preset: &str,
    count: u64,
    duration_secs: u64,
    kind: &str,
    telemetry_file: &str,
    telemetry: &[Value],
    renderer: &str,
) -> Value {
    let mut failures = Vec::new();
    let direct_backend = telemetry.iter().any(|sample| {
        sample
            .pointer("/launcher/present_backend")
            .and_then(Value::as_str)
            == Some("fpga-vblank-latch-hidden")
    });
    if !direct_backend {
        failures.push(json!({"kind": "direct-hidden-backend-missing"}));
    }
    let mut frame_map = std::collections::BTreeMap::new();
    for sample in telemetry {
        if let Some(recent) = sample
            .pointer("/launcher/frame_budget/recent_frames")
            .and_then(Value::as_array)
        {
            for frame in recent {
                let selected = frame.get("screensaver_active").and_then(Value::as_bool)
                    == Some(true)
                    && frame.get("screensaver_renderer").and_then(Value::as_str) == Some(renderer)
                    && frame.get("particle_preset").and_then(Value::as_str) == Some(preset)
                    && frame.get("particle_count").and_then(Value::as_u64) == Some(count);
                if selected && let Some(id) = frame.get("frame").and_then(Value::as_u64) {
                    frame_map.insert(id, frame.clone());
                }
            }
        }
    }
    let frames = frame_map.values().collect::<Vec<_>>();
    if frames.len() <= SCREENSAVER_STARTUP_WARMUP_FRAMES {
        failures.push(json!({"kind": "insufficient-particle-frames", "frames": frames.len()}));
    }
    for frame in &frames {
        if let Err(error) = validate_screensaver_frame_evidence(0, frame_u64(frame, "frame"), frame)
        {
            failures.push(json!({"kind": "invalid-frame-evidence", "detail": error.to_string()}));
            break;
        }
    }
    let steady = frames
        .get(SCREENSAVER_STARTUP_WARMUP_FRAMES..)
        .unwrap_or_default();
    for pair in steady.windows(2) {
        let previous = pair[0];
        let current = pair[1];
        let frame_id = frame_u64(current, "frame");
        if frame_id != frame_u64(previous, "frame").saturating_add(1) {
            failures.push(json!({"kind": "frame-gap", "frame": frame_id}));
        }
        if !presentation_sequence_is_contiguous(
            frame_u16(previous, "main_present_sequence"),
            frame_u16(current, "main_present_sequence"),
        ) {
            failures.push(json!({"kind": "sequence-gap", "frame": frame_id}));
        }
        if frame_u16(current, "main_present_flip_count")
            .wrapping_sub(frame_u16(previous, "main_present_flip_count"))
            != 1
        {
            failures.push(json!({"kind": "latch-flip-gap", "frame": frame_id}));
        }
        if frame_u16(current, "main_present_drop_count")
            .wrapping_sub(frame_u16(previous, "main_present_drop_count"))
            != 0
        {
            failures.push(json!({"kind": "latch-drop", "frame": frame_id}));
        }
    }
    let mut refresh_periods = steady
        .iter()
        .filter_map(|frame| frame.get("vsync_period_us").and_then(Value::as_u64))
        .filter(|period| *period > 0)
        .collect::<Vec<_>>();
    refresh_periods.sort_unstable();
    let refresh_period_us = median_u64(&refresh_periods).unwrap_or(16_667);
    let physical = if steady.len() >= 2 {
        match physical_refresh_summary(0, steady, refresh_period_us) {
            Ok(summary) => summary,
            Err(error) => {
                failures.push(
                    json!({"kind": "physical-refresh-evidence", "detail": error.to_string()}),
                );
                Value::Null
            }
        }
    } else {
        Value::Null
    };
    let unique_fps = physical
        .get("unique_fps")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let refresh_hz = physical
        .get("refresh_hz")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    if (unique_fps - refresh_hz).abs() > 0.1 {
        failures.push(json!({
            "kind": "unique-fps",
            "actual": unique_fps,
            "required": refresh_hz,
        }));
    }
    if physical
        .get("repeated_refreshes")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX)
        != 0
    {
        failures.push(json!({"kind": "repeated-refresh"}));
    }
    if physical
        .get("long_completion_intervals")
        .and_then(Value::as_array)
        .is_none_or(|intervals| !intervals.is_empty())
    {
        failures.push(json!({"kind": "long-completion-gap"}));
    }
    let phases = steady
        .iter()
        .filter_map(|frame| frame.get("particle_phase").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    if renderer == "particle-magik" {
        for required in ["static", "form", "hold", "disperse"] {
            if !phases.contains(required) {
                failures.push(json!({"kind": "missing-phase", "phase": required}));
            }
        }
    }
    if steady.iter().any(|frame| {
        frame.get("main_present_copy_path").and_then(Value::as_str) != Some("external-direct")
            || frame.get("main_present_status").and_then(Value::as_str) != Some("ok")
            || frame.get("main_present_pending").and_then(Value::as_bool) != Some(false)
            || frame.get("vsync_source").and_then(Value::as_str) != Some("vsync")
            || frame_u64(frame, "vsync_miss_streak") != 0
    }) {
        failures.push(json!({"kind": "presentation-path"}));
    }
    for (field, kind) in [
        (
            "screensaver_render_ahead_starvation_count",
            "render-starvation",
        ),
        ("screensaver_render_ahead_reused_frames", "reused-frame"),
        (
            "screensaver_render_ahead_superseded_frames",
            "superseded-frame",
        ),
    ] {
        if steady.iter().any(|frame| frame_u64(frame, field) != 0) {
            failures.push(json!({"kind": kind}));
        }
    }
    let mut render_wall = steady
        .iter()
        .map(|frame| frame_u64(frame, "screensaver_render_ahead_render_wall_us"))
        .collect::<Vec<_>>();
    render_wall.sort_unstable();
    let p99_render_wall_us = percentile_99(&render_wall);
    let deadline_us = refresh_period_us.saturating_sub(PARTICLE_POST_RESERVE_US);
    if p99_render_wall_us >= deadline_us {
        failures.push(json!({
            "kind": "render-deadline",
            "p99_us": p99_render_wall_us,
            "deadline_us": deadline_us,
        }));
    }
    if steady
        .iter()
        .all(|frame| frame_u64(frame, "particle_visible") == 0)
    {
        failures.push(json!({"kind": "no-visible-particles"}));
    }
    let phase_labels = steady
        .iter()
        .filter_map(|frame| frame.get("particle_phase").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let phase_timing = phase_labels
        .into_iter()
        .map(|phase| {
            let matching = steady
                .iter()
                .copied()
                .filter(|frame| frame.get("particle_phase").and_then(Value::as_str) == Some(phase))
                .collect::<Vec<_>>();
            (
                phase.to_string(),
                json!({
                    "frames": matching.len(),
                    "simulation_mean_us": mean_frame_field(&matching, "particle_simulation_us"),
                    "simulation_cpu_mean_us": mean_frame_field(
                        &matching,
                        "particle_simulation_cpu_us"
                    ),
                    "simulation_descheduled_mean_us": mean_frame_difference(
                        &matching,
                        "particle_simulation_us",
                        "particle_simulation_cpu_us"
                    ),
                    "projection_mean_us": mean_frame_field(&matching, "particle_projection_us"),
                    "projection_cpu_mean_us": mean_frame_field(
                        &matching,
                        "particle_projection_cpu_us"
                    ),
                    "projection_descheduled_mean_us": mean_frame_difference(
                        &matching,
                        "particle_projection_us",
                        "particle_projection_cpu_us"
                    ),
                    "preparation_wait_mean_us": mean_frame_field(
                        &matching,
                        "particle_preparation_wait_us"
                    ),
                    "prepared_frame_age_mean_us": mean_frame_field(
                        &matching,
                        "particle_prepared_frame_age_us"
                    ),
                    "worker_wake_latency_mean_us": mean_frame_field(
                        &matching,
                        "particle_worker_wake_latency_us"
                    ),
                    "clear_mean_us": mean_frame_field(&matching, "particle_clear_us"),
                    "clear_cpu_mean_us": mean_frame_field(&matching, "particle_clear_cpu_us"),
                    "clear_descheduled_mean_us": mean_frame_difference(
                        &matching,
                        "particle_clear_us",
                        "particle_clear_cpu_us"
                    ),
                    "raster_mean_us": mean_frame_field(&matching, "particle_raster_us"),
                    "raster_cpu_mean_us": mean_frame_field(&matching, "particle_raster_cpu_us"),
                    "raster_descheduled_mean_us": mean_frame_difference(
                        &matching,
                        "particle_raster_us",
                        "particle_raster_cpu_us"
                    ),
                    "render_wall_mean_us": mean_frame_field(
                        &matching,
                        "screensaver_render_ahead_render_wall_us"
                    ),
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    let simulation_bytes = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_simulation_bytes"))
        .max()
        .unwrap_or(0);
    let renderer_scratch_bytes = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_renderer_scratch_bytes"))
        .max()
        .unwrap_or(0);
    let pmu_cycles = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_cycles"))
        .sum::<u64>();
    let pmu_instructions = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_instructions"))
        .sum::<u64>();
    let pmu_cache_references = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_cache_references"))
        .sum::<u64>();
    let pmu_cache_misses = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_cache_misses"))
        .sum::<u64>();
    let pmu_branch_instructions = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_branch_instructions"))
        .sum::<u64>();
    let pmu_branch_misses = steady
        .iter()
        .map(|frame| frame_u64(frame, "particle_pmu_branch_misses"))
        .sum::<u64>();
    json!({
        "kind": kind,
        "preset": preset,
        "count": count,
        "duration_secs": duration_secs,
        "telemetry_file": telemetry_file,
        "frames": steady.len(),
        "qualified": failures.is_empty(),
        "failures": failures,
        "refresh_period_us": refresh_period_us,
        "render_deadline_us": deadline_us,
        "p99_render_wall_us": p99_render_wall_us,
        "max_render_wall_us": render_wall.last().copied().unwrap_or(0),
        "physical_refresh": physical,
        "phase_timing": phase_timing,
        "scheduler": {
            "voluntary_context_switches": steady.iter().map(|frame| {
                frame_u64(frame, "particle_voluntary_context_switches")
            }).sum::<u64>(),
            "involuntary_context_switches": steady.iter().map(|frame| {
                frame_u64(frame, "particle_involuntary_context_switches")
            }).sum::<u64>(),
            "cpu_migrations": steady.iter().filter(|frame| {
                frame_u64(frame, "particle_render_cpu_start")
                    != frame_u64(frame, "particle_render_cpu_end")
            }).count(),
        },
        "pmu": {
            "available_frames": steady.iter().filter(|frame| {
                frame.get("particle_pmu_available").and_then(Value::as_bool) == Some(true)
            }).count(),
            "cycles": pmu_cycles,
            "instructions": pmu_instructions,
            "instructions_per_cycle": ratio(pmu_instructions, pmu_cycles),
            "cache_references": pmu_cache_references,
            "cache_misses": pmu_cache_misses,
            "cache_miss_pct": ratio(pmu_cache_misses, pmu_cache_references) * 100.0,
            "branch_instructions": pmu_branch_instructions,
            "branch_misses": pmu_branch_misses,
            "branch_miss_pct": ratio(pmu_branch_misses, pmu_branch_instructions) * 100.0,
        },
        "simulation_backends": steady.iter().filter_map(|frame| {
            frame.get("particle_simulation_backend").and_then(Value::as_str)
        }).collect::<std::collections::BTreeSet<_>>(),
        "projection_backends": steady.iter().filter_map(|frame| {
            frame.get("particle_projection_backend").and_then(Value::as_str)
        }).collect::<std::collections::BTreeSet<_>>(),
        "pipeline": {
            "preparation_wait_mean_us": mean_frame_field(
                steady,
                "particle_preparation_wait_us"
            ),
            "preparation_wait_p99_us": percentile_99_frame_field(
                steady,
                "particle_preparation_wait_us"
            ),
            "preparation_wait_max_us": max_frame_field(
                steady,
                "particle_preparation_wait_us"
            ),
            "prepared_frame_age_mean_us": mean_frame_field(
                steady,
                "particle_prepared_frame_age_us"
            ),
            "prepared_frame_age_p99_us": percentile_99_frame_field(
                steady,
                "particle_prepared_frame_age_us"
            ),
            "prepared_frame_age_max_us": max_frame_field(
                steady,
                "particle_prepared_frame_age_us"
            ),
            "lookahead_mismatch_count": sum_frame_field(
                steady,
                "particle_lookahead_mismatch_count"
            ),
            "queue_depth_mean": mean_frame_field(
                steady,
                "particle_preparation_queue_depth"
            ),
            "queue_depth_max": max_frame_field(
                steady,
                "particle_preparation_queue_depth"
            ),
            "worker_wake_latency_mean_us": mean_frame_field(
                steady,
                "particle_worker_wake_latency_us"
            ),
            "worker_wake_latency_p99_us": percentile_99_frame_field(
                steady,
                "particle_worker_wake_latency_us"
            ),
            "worker_wake_latency_max_us": max_frame_field(
                steady,
                "particle_worker_wake_latency_us"
            ),
        },
        "cpu": {
            "preparation_pct_of_one_core": (
                mean_frame_field(steady, "particle_simulation_cpu_us")
                    + mean_frame_field(steady, "particle_projection_cpu_us")
            ) * 100.0 / refresh_period_us.max(1) as f64,
            "clear_raster_pct_of_one_core": (
                mean_frame_field(steady, "particle_clear_cpu_us")
                    + mean_frame_field(steady, "particle_raster_cpu_us")
            ) * 100.0 / refresh_period_us.max(1) as f64,
            "renderer_pct_of_one_core": mean_frame_field(
                steady,
                "screensaver_render_ahead_render_cpu_us"
            ) * 100.0 / refresh_period_us.max(1) as f64,
            "process_pct_of_one_core": mean_frame_field(steady, "process_cpu_us")
                * 100.0 / refresh_period_us.max(1) as f64,
        },
        "visible": {
            "mean": mean_frame_field(steady, "particle_visible"),
            "minimum": steady
                .iter()
                .map(|frame| frame_u64(frame, "particle_visible"))
                .min()
                .unwrap_or(0),
        },
        "memory": {
            "simulation_bytes": simulation_bytes,
            "renderer_scratch_bytes": renderer_scratch_bytes,
            "total_bytes": simulation_bytes.saturating_add(renderer_scratch_bytes),
            "simulation_bytes_per_particle": simulation_bytes / count.max(1),
            "renderer_scratch_bytes_per_particle": renderer_scratch_bytes / count.max(1),
        },
    })
}

fn capture_particle_phases(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    count: u64,
) -> Result<Value> {
    // A framebuffer PNG can consume enough of the early Hold window that the
    // next matching sample belongs to the following animation cycle.
    const CAPTURE_STATE_TIMEOUT: Duration = Duration::from_secs(24);
    let session = connect_with(&config.connection, 10)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_SCREENSAVER_START_ACTIVE".into(), "1".into()),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-magik".into(),
                ),
                ("MISTER_PARTICLE_COUNT".into(), count.to_string()),
                ("MISTER_PARTICLE_PRESET".into(), "visual".into()),
                ("MISTER_PARTICLE_SEED".into(), "827141709451".into()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);
    wait_for_particle_capture_state(&config.agent, count, "static", 0..=0, CAPTURE_STATE_TIMEOUT)?;
    let static_capture = request_framebuffer_png_at(&config.agent)?;
    let static_path = output_dir.join("particle-static.png");
    fs::write(&static_path, &static_capture.png)?;
    wait_for_particle_capture_state(
        &config.agent,
        count,
        "hold",
        0..=15_000,
        CAPTURE_STATE_TIMEOUT,
    )?;
    let formed_capture = request_framebuffer_png_at(&config.agent)?;
    let formed_path = output_dir.join("particle-formed.png");
    fs::write(&formed_path, &formed_capture.png)?;
    wait_for_particle_capture_state(
        &config.agent,
        count,
        "hold",
        30_000..=60_000,
        CAPTURE_STATE_TIMEOUT,
    )?;
    let rotated_capture = request_framebuffer_png_at(&config.agent)?;
    let rotated_path = output_dir.join("particle-rotated.png");
    fs::write(&rotated_path, &rotated_capture.png)?;
    Ok(json!({
        "static": static_path,
        "formed": formed_path,
        "rotated": rotated_path,
        "count": count,
        "source": capture_source_label(&rotated_capture.result)?,
    }))
}

fn wait_for_particle_capture_state(
    endpoint: &AgentEndpoint,
    count: u64,
    phase: &str,
    rotation_y_millidegrees: std::ops::RangeInclusive<u64>,
    timeout: Duration,
) -> Result<()> {
    const CAPTURE_POLL_DURATION: Duration = Duration::from_millis(100);
    let started = Instant::now();
    while started.elapsed() < timeout {
        let telemetry = agent_telemetry_for_duration(endpoint, CAPTURE_POLL_DURATION)?;
        if particle_capture_state_seen(&telemetry, count, phase, rotation_y_millidegrees.clone()) {
            return Ok(());
        }
    }
    Err(format!(
        "particle capture did not reach phase={phase} rotation={}..={} within {}ms",
        rotation_y_millidegrees.start(),
        rotation_y_millidegrees.end(),
        timeout.as_millis()
    )
    .into())
}

fn particle_capture_state_seen(
    telemetry: &[Value],
    count: u64,
    phase: &str,
    rotation_y_millidegrees: std::ops::RangeInclusive<u64>,
) -> bool {
    telemetry
        .iter()
        .filter_map(|sample| {
            sample
                .pointer("/launcher/frame_budget/recent_frames")
                .and_then(Value::as_array)
        })
        .flatten()
        .filter(|frame| {
            frame.get("screensaver_active").and_then(Value::as_bool) == Some(true)
                && frame.get("screensaver_renderer").and_then(Value::as_str)
                    == Some("particle-magik")
                && frame.get("particle_preset").and_then(Value::as_str) == Some("visual")
                && frame.get("particle_count").and_then(Value::as_u64) == Some(count)
        })
        .max_by_key(|frame| frame.get("frame").and_then(Value::as_u64).unwrap_or(0))
        .is_some_and(|frame| {
            frame.get("particle_phase").and_then(Value::as_str) == Some(phase)
                && frame
                    .get("particle_rotation_y_millidegrees")
                    .and_then(Value::as_u64)
                    .is_some_and(|angle| rotation_y_millidegrees.contains(&angle))
        })
}

fn validate_particle_display_geometry(session: &Session) -> Result<()> {
    let framebuffer = remote_read(session, "/sys/class/graphics/fb0/virtual_size")
        .ok_or("device framebuffer size is unavailable")?;
    let bits_per_pixel = remote_read(session, "/sys/class/graphics/fb0/bits_per_pixel")
        .ok_or("device framebuffer depth is unavailable")?;
    if framebuffer.trim().replace(',', "x") != "960x540" || bits_per_pixel.trim() != "16" {
        return Err(format!(
            "particle benchmark display is {} at {} bpp, expected 960x540 at 16 bpp",
            framebuffer.trim().replace(',', "x"),
            bits_per_pixel.trim()
        )
        .into());
    }
    Ok(())
}

fn verify_particle_benchmark_restoration(
    config: &NativeDeviceConfig,
    original_mode: &str,
    original_ini: &str,
    expected_boot_id: &str,
    expected_manifest: &str,
) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    let state = exec_checked_output(
        &session,
        "verify restored particle benchmark display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(state.stdout.trim())?.is_some()
        || parse_display_reply_active(state.stdout.trim())? != original_mode
    {
        return Err("particle benchmark did not restore the original display mode".into());
    }
    let final_boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable after particle benchmark")?;
    if final_boot_id.trim() != expected_boot_id {
        return Err("device rebooted during the particle benchmark".into());
    }
    let final_manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing after particle benchmark")?;
    if final_manifest != expected_manifest {
        return Err("installed platform manifest changed during particle benchmark".into());
    }
    let final_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    if final_ini != original_ini {
        return Err("MiSTer.ini changed while restoring the particle benchmark".into());
    }
    exec_checked(
        &session,
        "post-particle-benchmark platform fingerprints",
        &installed_platform_verify_command(Layout::Development),
    )?;
    exec_checked(
        &session,
        "post-particle-benchmark delivery health",
        &delivery_health_command("dev")?,
    )?;
    Ok(())
}

fn persist_and_qualify_particle_benchmark(
    output_dir: &Path,
    summary: &Value,
    run: ParticleBenchmarkRun,
) -> Result<String> {
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(summary)?),
    )?;
    fs::write(
        output_dir.join("report.md"),
        particle_benchmark_report(summary),
    )?;
    let failed = match run {
        ParticleBenchmarkRun::Complete => ["capacity", "visual"].into_iter().any(|preset| {
            summary
                .pointer(&format!("/presets/{preset}/confirmation/qualified"))
                .and_then(Value::as_bool)
                != Some(true)
        }),
        ParticleBenchmarkRun::Capacity => {
            summary
                .pointer("/presets/capacity/confirmation/qualified")
                .and_then(Value::as_bool)
                != Some(true)
        }
        ParticleBenchmarkRun::Demo40k => {
            summary.pointer("/demo/qualified").and_then(Value::as_bool) != Some(true)
        }
        ParticleBenchmarkRun::Step => {
            summary.pointer("/step/qualified").and_then(Value::as_bool) != Some(true)
        }
        ParticleBenchmarkRun::Showcase(_) => {
            summary.pointer("/demo/qualified").and_then(Value::as_bool) != Some(true)
        }
    };
    if failed {
        return Err(format!(
            "particle benchmark did not qualify; evidence retained at {}",
            output_dir.display()
        )
        .into());
    }
    serde_json::to_string(summary).map_err(Into::into)
}

fn particle_benchmark_report(summary: &Value) -> String {
    if let Some(demo) = summary.get("demo").filter(|demo| !demo.is_null()) {
        let showcase = summary.get("schema").and_then(Value::as_str)
            == Some("mister-magik-particle-showcase-v1");
        let title = if showcase {
            "Particle Showcase Trial"
        } else {
            "Particle 40K Visual Trial"
        };
        let preset = demo
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let mut report = format!(
            "# {title}\n\n- Geometry: 960x540 RGB565\n- Presentation: direct hidden-slot latch\n- Preset/demo: {preset}\n- Particles: {}\n- Duration: {} seconds\n- Qualified: {}\n- Unique FPS: {:.6}\n- Repeated refreshes: {}\n- Process CPU: {:.2}% of one core\n- Update+projection CPU: {:.2}% of one core\n- Clear+raster CPU: {:.2}% of one core\n- Renderer CPU: {:.2}% of one core\n- Preparation wait mean / P99 / max: {:.2} / {} / {} us\n- Prepared-frame age mean / P99 / max: {:.2} / {} / {} us\n- Worker wake latency mean / P99 / max: {:.2} / {} / {} us\n- Lookahead mismatch recomputes: {}\n- Preparation queue depth mean / max: {:.2} / {}\n- P99 render wall: {} us\n- Maximum render wall: {} us\n\n## Phase means\n\n| Phase | Simulation wall | Simulation CPU | Projection wall | Projection CPU | Preparation wait | Clear wall | Raster wall | Render wall |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
            demo.get("count").and_then(Value::as_u64).unwrap_or(0),
            demo.get("duration_secs")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.get("qualified")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            demo.pointer("/physical_refresh/unique_fps")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/physical_refresh/repeated_refreshes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/cpu/process_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/cpu/preparation_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/cpu/clear_raster_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/cpu/renderer_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/pipeline/preparation_wait_mean_us")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/pipeline/preparation_wait_p99_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/preparation_wait_max_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/prepared_frame_age_mean_us")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/pipeline/prepared_frame_age_p99_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/prepared_frame_age_max_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/worker_wake_latency_mean_us")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/pipeline/worker_wake_latency_p99_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/worker_wake_latency_max_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/lookahead_mismatch_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.pointer("/pipeline/queue_depth_mean")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            demo.pointer("/pipeline/queue_depth_max")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.get("p99_render_wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            demo.get("max_render_wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        let empty = serde_json::Map::new();
        let phases = demo
            .get("phase_timing")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        for (phase, timing) in phases {
            let mean = |field| timing.get(field).and_then(Value::as_f64).unwrap_or(0.0);
            report.push_str(&format!(
                "| {phase} | {:.2} us | {:.2} us | {:.2} us | {:.2} us | {:.2} us | {:.2} us | {:.2} us | {:.2} us |\n",
                mean("simulation_mean_us"),
                mean("simulation_cpu_mean_us"),
                mean("projection_mean_us"),
                mean("projection_cpu_mean_us"),
                mean("preparation_wait_mean_us"),
                mean("clear_mean_us"),
                mean("raster_mean_us"),
                mean("render_wall_mean_us"),
            ));
        }
        return report;
    }
    if let Some(step) = summary.get("step").filter(|step| !step.is_null()) {
        return format!(
            "# Particle Optimisation Trial\n\n- Geometry: 960x540 RGB565\n- Presentation: direct hidden-slot latch\n- Preset: capacity\n- Particles: {}\n- Qualified: {}\n- Unique FPS: {:.6}\n- Process CPU: {:.2}% of one core\n- Renderer CPU: {:.2}% of one core\n- P99 render wall: {} us\n- Maximum render wall: {} us\n",
            step.get("count").and_then(Value::as_u64).unwrap_or(0),
            step.get("qualified")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            step.pointer("/physical_refresh/unique_fps")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            step.pointer("/cpu/process_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            step.pointer("/cpu/renderer_pct_of_one_core")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            step.get("p99_render_wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            step.get("max_render_wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
    let mut report = String::from(
        "# Particle Capacity Benchmark\n\n- Geometry: 960x540 RGB565\n- Presentation: direct hidden-slot latch\n\n## Confirmed ceilings\n\n",
    );
    for preset in ["capacity", "visual"] {
        if summary
            .pointer(&format!("/presets/{preset}"))
            .is_none_or(Value::is_null)
        {
            continue;
        }
        let count = summary
            .pointer(&format!("/presets/{preset}/confirmed_count"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let first_fail = summary
            .pointer(&format!("/presets/{preset}/first_failing_count"))
            .and_then(Value::as_u64)
            .map_or_else(|| "none within bound".into(), |value| value.to_string());
        let simulation_bytes = summary
            .pointer(&format!("/presets/{preset}/memory/simulation_bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let scratch_bytes = summary
            .pointer(&format!("/presets/{preset}/memory/renderer_scratch_bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        report.push_str(&format!(
            "- {preset}: {count} particles; first failing count: {first_fail}; simulation memory: {simulation_bytes} bytes; renderer scratch: {scratch_bytes} bytes\n"
        ));
    }
    report
}

fn profile_installed_screensaver(config: &NativeDeviceConfig, output_dir: &Path) -> Result<String> {
    const DISPLAY_MODE: &str = "hdmi-1280x720p60";
    let benchmark_mode = DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == DISPLAY_MODE)
        .copied()
        .ok_or("screensaver benchmark display mode is unavailable")?;
    let session = connect_with(&config.connection, 10)?;
    let capability = exec_checked_output(
        &session,
        "installed benchmark capability",
        "/media/fat/mister-magik-dev/mister-magik-fb benchmark-capabilities",
    )?;
    let capability = last_json_line(&capability.stdout)
        .ok_or("installed benchmark capability output contains no JSON report")?;
    if capability
        .get("screensaver-pprof-v1")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("installed app does not support screensaver-pprof-v1".into());
    }
    if capability
        .get("screensaver-frame-evidence-v3")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("installed app does not support screensaver-frame-evidence-v3".into());
    }
    let initial_status = read_launcher_status(&session)?;
    if initial_status.get("catalog_ready").and_then(Value::as_bool) != Some(true)
        || initial_status
            .get("catalog_games")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
    {
        return Err("screensaver benchmark requires an existing usable cached catalog".into());
    }
    let manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing")?;
    let boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable")?
        .trim()
        .to_string();
    let original_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    let original_ini_sha256 = encode_hex(&Sha256::digest(original_ini.as_bytes()));
    let original_reply = exec_checked_output(
        &session,
        "query original benchmark display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let original_mode = parse_display_reply_active(original_reply.stdout.trim())?;
    if parse_display_reply_pending(original_reply.stdout.trim())?.is_some() {
        return Err("screensaver benchmark cannot start during a display transaction".into());
    }
    fs::create_dir_all(output_dir)?;
    drop(session);
    let _signal_guard = ScreensaverProfileSignalGuard::install();
    let mut benchmark_ini = None;

    let run_result = (|| -> Result<(Vec<Value>, String, String, String)> {
        apply_confirmed_benchmark_display_mode(config, benchmark_mode)?;
        let session = connect_with(&config.connection, 10)?;
        benchmark_ini = Some(
            remote_read(&session, "/media/fat/MiSTer.ini")
                .ok_or("MiSTer.ini is unavailable after selecting benchmark mode")?,
        );
        let target_status = read_launcher_status(&session)?;
        let framebuffer_size = remote_read(&session, "/sys/class/graphics/fb0/virtual_size")
            .ok_or("device framebuffer size is unavailable")?
            .trim()
            .replace(',', "x");
        let framebuffer_bits_per_pixel =
            remote_read(&session, "/sys/class/graphics/fb0/bits_per_pixel")
                .ok_or("device framebuffer depth is unavailable")?
                .trim()
                .to_string();
        let output_route = target_status
            .get("output_route")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if framebuffer_size != "1280x720" || framebuffer_bits_per_pixel != "16" {
            return Err(format!(
                "screensaver benchmark display is {framebuffer_size} at {framebuffer_bits_per_pixel} bpp, expected 1280x720 at 16 bpp"
            )
            .into());
        }
        drop(session);
        let mut summaries = Vec::new();
        for run in 1..=1 {
            if screensaver_profile_interrupted() {
                return Err("screensaver benchmark interrupted".into());
            }
            summaries.push(profile_installed_screensaver_run(config, output_dir, run)?);
        }
        Ok((
            summaries,
            framebuffer_size,
            framebuffer_bits_per_pixel,
            output_route,
        ))
    })();
    let launcher_cleanup = restore_installed_screensaver_profile(config);
    let final_verification = finalize_benchmark_state(
        config,
        benchmark_mode,
        benchmark_ini.as_deref(),
        &boot_id,
        &manifest,
    );
    let cleanup_result =
        combine_benchmark_cleanup(launcher_cleanup, final_verification.map(|_| ()));
    let (summaries, framebuffer_size, framebuffer_bits_per_pixel, output_route) =
        match (run_result, cleanup_result) {
            (Ok(result), Ok(())) => result,
            (Err(error), Ok(())) => return Err(error),
            (Ok(_), Err(error)) => {
                return Err(format!("screensaver benchmark cleanup failed: {error}").into());
            }
            (Err(run_error), Err(cleanup_error)) => {
                return Err(format!(
                    "{run_error}; screensaver benchmark cleanup failed: {cleanup_error}"
                )
                .into());
            }
        };
    let benchmark_ini = benchmark_ini.ok_or("benchmark mode INI evidence is unavailable")?;
    let benchmark_ini_sha256 = encode_hex(&Sha256::digest(benchmark_ini.as_bytes()));
    let summary = json!({
        "schema": "mister-magik-installed-screensaver-benchmark-v4",
        "benchmark_contract": {
            "startup_warmup_frames": SCREENSAVER_STARTUP_WARMUP_FRAMES,
            "startup_frames_are_informational": true,
            "steady_state_requires_every_physical_refresh": true,
            "wall_overruns_are_informational": true,
            "retains_benchmark_display": true,
            "rationale": "screensaver activation may be late without being visible; once running every physical refresh must latch one unique frame",
        },
        "boot_id": boot_id,
        "manifest": parse_manifest_evidence(&manifest),
        "display": {
            "benchmark_mode": benchmark_mode.id,
            "original_mode": original_mode,
            "final_mode": benchmark_mode.id,
            "retained": true,
            "output_route": output_route,
            "framebuffer": framebuffer_size,
            "bits_per_pixel": framebuffer_bits_per_pixel,
            "original_ini_sha256": original_ini_sha256,
            "benchmark_ini_sha256": benchmark_ini_sha256,
            "final_ini_sha256": benchmark_ini_sha256,
        },
        "runs": summaries,
        "output_dir": output_dir,
    });
    persist_and_qualify_screensaver_benchmark(output_dir, &summary)
}

fn persist_and_qualify_screensaver_benchmark(output_dir: &Path, summary: &Value) -> Result<String> {
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(summary)?),
    )?;
    fs::write(
        output_dir.join("report.md"),
        screensaver_benchmark_report(summary)?,
    )?;
    let qualification_failures = summary
        .get("runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|run| {
            let failures = run
                .pointer("/qualification/failures")
                .and_then(Value::as_array)?;
            (!failures.is_empty()).then_some(failures.len())
        })
        .sum::<usize>();
    if qualification_failures > 0 {
        return Err(format!(
            "screensaver benchmark failed {qualification_failures} qualification gate(s); evidence retained at {}",
            output_dir.display()
        )
        .into());
    }
    serde_json::to_string(&summary).map_err(Into::into)
}

fn apply_confirmed_benchmark_display_mode(
    config: &NativeDeviceConfig,
    mode: DisplayMatrixMode,
) -> Result<()> {
    apply_confirmed_display_mode(config, mode, "screensaver benchmark")
}

fn apply_confirmed_display_mode(
    config: &NativeDeviceConfig,
    mode: DisplayMatrixMode,
    operation: &str,
) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    let current = exec_checked_output(
        &session,
        &format!("query {operation} display mode"),
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(current.stdout.trim())?.is_some() {
        return Err(format!("{operation} display transaction is already pending").into());
    }
    let active = parse_display_reply_active(current.stdout.trim())?;
    let ready = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    if active == mode.id {
        validate_live_display_mode(&session, mode)?;
        return Ok(());
    }
    exec_checked(
        &session,
        &format!("apply {operation} display mode"),
        &acknowledged_main_command(&format!(
            "mister_magik_display_apply_headless_v1 mode={}",
            mode.id
        )),
    )?;
    drop(session);
    let session = connect_with(&config.connection, 10)?;
    wait_launcher_ready_after(
        &session,
        ready.launcher_pid,
        Instant::now(),
        Duration::from_secs(15),
    )?;
    validate_live_display_mode(&session, mode)?;
    exec_checked(
        &session,
        &format!("confirm {operation} display mode"),
        &acknowledged_main_command("mister_magik_display_confirm_v1"),
    )?;
    wait_display_transaction_idle(&session, Duration::from_secs(15))
}

fn cancel_pending_benchmark_display_mode(config: &NativeDeviceConfig) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    let ready = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(15))?;
    let state = exec_checked_output(
        &session,
        "query incomplete benchmark display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(state.stdout.trim())?.is_some() {
        exec_checked(
            &session,
            "cancel pending benchmark display mode",
            &acknowledged_main_command("mister_magik_display_cancel_v1"),
        )?;
        drop(session);
        let session = connect_with(&config.connection, 10)?;
        wait_launcher_ready_after(
            &session,
            ready.launcher_pid,
            Instant::now(),
            Duration::from_secs(15),
        )?;
    }
    Ok(())
}

fn finalize_benchmark_state(
    config: &NativeDeviceConfig,
    benchmark_mode: DisplayMatrixMode,
    expected_ini: Option<&str>,
    expected_boot_id: &str,
    expected_manifest: &str,
) -> Result<String> {
    let mut session = connect_with(&config.connection, 10)?;
    wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
    let mut state = exec_checked_output(
        &session,
        "verify retained benchmark display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    if parse_display_reply_pending(state.stdout.trim())?.is_some() {
        drop(session);
        cancel_pending_benchmark_display_mode(config)?;
        session = connect_with(&config.connection, 10)?;
        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))?;
        state = exec_checked_output(
            &session,
            "verify cancelled benchmark display transaction",
            &acknowledged_main_command("mister_magik_display_get_v1"),
        )?;
        if parse_display_reply_pending(state.stdout.trim())?.is_some() {
            return Err("screensaver benchmark left a pending display transaction".into());
        }
    }
    let active = parse_display_reply_active(state.stdout.trim())?;
    if expected_ini.is_some() && active != benchmark_mode.id {
        return Err(format!(
            "screensaver benchmark ended in {active}, expected {}",
            benchmark_mode.id
        )
        .into());
    }
    if active == benchmark_mode.id {
        validate_benchmark_display_geometry(&session)?;
    }
    let final_boot_id = remote_read(&session, "/proc/sys/kernel/random/boot_id")
        .ok_or("device boot id is unavailable after benchmark")?;
    if final_boot_id.trim() != expected_boot_id {
        return Err("device rebooted during the in-place screensaver benchmark".into());
    }
    let final_manifest = remote_read(&session, "/media/fat/mister-magik-dev/platform-v3.manifest")
        .ok_or("development platform manifest is missing after benchmark")?;
    if final_manifest != expected_manifest {
        return Err("installed platform manifest changed during benchmark".into());
    }
    exec_checked(
        &session,
        "post-benchmark platform fingerprints",
        &installed_platform_verify_command(Layout::Development),
    )?;
    let final_ini =
        remote_read(&session, "/media/fat/MiSTer.ini").ok_or("MiSTer.ini is unavailable")?;
    if let Some(expected_ini) = expected_ini
        && final_ini != expected_ini
    {
        return Err("MiSTer.ini changed after the confirmed 720p benchmark baseline".into());
    }
    exec_checked(
        &session,
        "post-benchmark delivery health",
        &delivery_health_command("dev")?,
    )?;
    Ok(encode_hex(&Sha256::digest(final_ini.as_bytes())))
}

fn validate_benchmark_display_geometry(session: &Session) -> Result<()> {
    let framebuffer = remote_read(session, "/sys/class/graphics/fb0/virtual_size")
        .ok_or("device framebuffer size is unavailable")?;
    let bits_per_pixel = remote_read(session, "/sys/class/graphics/fb0/bits_per_pixel")
        .ok_or("device framebuffer depth is unavailable")?;
    if framebuffer.trim().replace(',', "x") != "1280x720" || bits_per_pixel.trim() != "16" {
        return Err(format!(
            "screensaver benchmark display is {} at {} bpp, expected 1280x720 at 16 bpp",
            framebuffer.trim().replace(',', "x"),
            bits_per_pixel.trim()
        )
        .into());
    }
    Ok(())
}

fn combine_benchmark_cleanup(first: Result<()>, second: Result<()>) -> Result<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => {
            Err(format!("{first}; final-state verification failed: {second}").into())
        }
    }
}

fn profile_installed_screensaver_run(
    config: &NativeDeviceConfig,
    output_dir: &Path,
    run: usize,
) -> Result<Value> {
    let remote_svg = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/run-{run}.svg");
    let remote_folded = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/run-{run}.folded");
    let remote_complete = format!("{SCREENSAVER_PROFILE_REMOTE_DIR}/run-{run}.json");
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "reset screensaver profile artifacts",
        &format!(
            "set -eu; mkdir -p {0}; rm -f {1} {2} {3}",
            sh(SCREENSAVER_PROFILE_REMOTE_DIR),
            sh(&remote_svg),
            sh(&remote_folded),
            sh(&remote_complete)
        ),
    )?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_LAUNCHER_START_SCREEN".into(), "home".into()),
                (
                    "MISTER_LAUNCHER_INPUT_SCRIPT".into(),
                    "up,a,down,a,down,down,a".into(),
                ),
                (
                    "MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES".into(),
                    "60".into(),
                ),
                ("MISTER_PPROF".into(), "1".into()),
                ("MISTER_PPROF_TRIGGER".into(), "screensaver".into()),
                (
                    "MISTER_PPROF_DURATION_SECS".into(),
                    SCREENSAVER_PROFILE_DURATION_SECS.to_string(),
                ),
                ("MISTER_PPROF_HZ".into(), "99".into()),
                ("MISTER_PPROF_OUT".into(), remote_svg.clone()),
                ("MISTER_PPROF_FOLDED_OUT".into(), remote_folded.clone()),
                ("MISTER_PPROF_COMPLETE".into(), remote_complete.clone()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    drop(session);

    let telemetry = agent_telemetry_until_screensaver_profile_complete(
        &config.agent,
        Duration::from_secs(SCREENSAVER_PROFILE_TIMEOUT_SECS),
    )?;
    let session = connect_with(&config.connection, 10)?;
    let metadata = remote_read(&session, &remote_complete)
        .ok_or("screensaver profile completion metadata is missing")?;
    let metadata_value: Value = serde_json::from_str(metadata.trim())?;
    if metadata_value.get("state").and_then(Value::as_str) != Some("complete")
        || metadata_value
            .get("sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            <= 0
    {
        return Err(format!("screensaver profile run {run} produced no CPU samples").into());
    }
    let svg = remote_read(&session, &remote_svg)
        .filter(|text| !text.is_empty())
        .ok_or("screensaver profile SVG is missing")?;
    let folded = remote_read(&session, &remote_folded)
        .filter(|text| !text.is_empty())
        .ok_or("screensaver profile folded stacks are missing")?;
    fs::write(output_dir.join(format!("run-{run}.svg")), svg)?;
    fs::write(output_dir.join(format!("run-{run}.folded")), folded)?;
    fs::write(
        output_dir.join(format!("run-{run}-profile.json")),
        format!("{}\n", serde_json::to_string_pretty(&metadata_value)?),
    )?;
    let telemetry_text = telemetry
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(
        output_dir.join(format!("run-{run}-telemetry.jsonl")),
        format!("{telemetry_text}\n"),
    )?;
    match summarize_screensaver_telemetry(run, &telemetry, metadata_value.clone()) {
        Ok(summary) => Ok(summary),
        Err(error) => Ok(json!({
            "run": run,
            "captured_frames": telemetry.len(),
            "profile": metadata_value,
            "qualification": {
                "qualified": false,
                "failures": [{
                    "kind": "missing-evidence",
                    "detail": error.to_string(),
                }],
            },
        })),
    }
}

fn restart_launcher_with_one_shot_env(
    session: &Session,
    options: LauncherRestartOptions,
) -> Result<()> {
    if options.clear_env || options.env_vars.is_empty() {
        return Err("one-shot launcher restart requires environment variables".into());
    }
    let previous = wait_launcher_ready(session, Instant::now(), Duration::from_secs(5))?;
    let parent = remote_parent_dir(&options.remote_env)?;
    let out = exec(session, &create_dir_command(parent), true)?;
    if let Some(error) = exec_failure_message("create one-shot launcher env parent", &out) {
        return Err(error.into());
    }
    put_bytes(
        session,
        &options.remote_env,
        one_shot_launcher_env_text(&options.env_vars, &options.remote_env).as_bytes(),
    )?;
    let started = Instant::now();
    let restart_result = issue_launcher_restart(session).and_then(|()| {
        wait_launcher_ready_after(
            session,
            previous.launcher_pid,
            started,
            Duration::from_secs(options.timeout_secs),
        )
        .map(|_| ())
    });
    let clear_result = prepare_launcher_env(
        session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: options.remote_env.clone(),
            ..LauncherRestartOptions::default()
        },
    );
    match (restart_result, clear_result) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(error), Ok(_)) => Err(error),
        (Ok(()), Err(error)) => {
            Err(format!("one-shot launcher env cleanup failed: {error}").into())
        }
        (Err(restart_error), Err(clear_error)) => Err(format!(
            "{restart_error}; one-shot launcher env cleanup failed: {clear_error}"
        )
        .into()),
    }
}

fn one_shot_launcher_env_text(vars: &[(String, String)], remote_env: &str) -> String {
    let mut text = launcher_env_text(vars);
    text.push_str("rm -f ");
    text.push_str(&shell_export_quote(remote_env));
    text.push('\n');
    text
}

fn restore_installed_screensaver_profile(config: &NativeDeviceConfig) -> Result<()> {
    let session = connect_with(&config.connection, 10)?;
    let cleanup = format!(
        "set -eu; rm -f {env} /tmp/mister-magik/realtime-frame-analytics; rm -rf {profiles}",
        env = sh(DEVELOPMENT_LAUNCHER_ENV_REMOTE),
        profiles = sh(SCREENSAVER_PROFILE_REMOTE_DIR)
    );
    let cleanup_result = exec_checked(&session, "screensaver benchmark cleanup", &cleanup);
    let restart_result = launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    );
    let final_cleanup_result = exec_checked(
        &session,
        "screensaver benchmark final cleanup",
        &format!(
            "{cleanup}; test ! -e {env}; test ! -e {profiles}; for delay in 1 1 1; do rm -f /tmp/mister-magik/realtime-frame-analytics; sleep \"$delay\"; done; test ! -e /tmp/mister-magik/realtime-frame-analytics",
            env = sh(DEVELOPMENT_LAUNCHER_ENV_REMOTE),
            profiles = sh(SCREENSAVER_PROFILE_REMOTE_DIR)
        ),
    );
    cleanup_result?;
    restart_result?;
    final_cleanup_result
}

fn read_launcher_status(session: &Session) -> Result<Value> {
    let text = remote_read(session, SLINT_STATUS_REMOTE).ok_or("launcher status is missing")?;
    serde_json::from_str(&text).map_err(Into::into)
}

fn parse_manifest_evidence(manifest: &str) -> Value {
    let values = manifest
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| {
            matches!(
                *key,
                "magik_revision"
                    | "gui_sha256"
                    | "main_sha256"
                    | "scanout_module_sha256"
                    | "latch_rbf_sha256"
            )
        })
        .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
        .collect();
    Value::Object(values)
}

fn summarize_screensaver_telemetry(
    run: usize,
    telemetry: &[Value],
    metadata: Value,
) -> Result<Value> {
    let active = telemetry
        .iter()
        .filter(|sample| {
            matches!(
                sample
                    .pointer("/launcher/screensaver_profile_state")
                    .and_then(Value::as_str),
                Some("active" | "complete")
            )
        })
        .collect::<Vec<_>>();
    if active.is_empty() {
        return Err(format!("screensaver profile run {run} has no active telemetry").into());
    }
    if active.iter().any(|sample| {
        sample
            .pointer("/launcher/catalog_refresh_policy")
            .and_then(Value::as_str)
            != Some("off")
            || sample
                .pointer("/launcher/catalog_worker_enabled")
                .and_then(Value::as_bool)
                != Some(false)
    }) {
        return Err(
            format!("screensaver profile run {run} did not disable catalog refresh").into(),
        );
    }
    if active.iter().any(|sample| {
        sample
            .pointer("/launcher/present_backend")
            .and_then(Value::as_str)
            != Some("fpga-vblank-latch-hidden")
            || sample
                .pointer("/launcher/present_status")
                .and_then(Value::as_str)
                != Some("ok")
    }) {
        return Err(
            format!("screensaver profile run {run} left the production present path").into(),
        );
    }

    let mut frames = std::collections::BTreeMap::new();
    for sample in &active {
        if let Some(recent) = sample
            .pointer("/launcher/frame_budget/recent_frames")
            .and_then(Value::as_array)
        {
            for frame in recent {
                if let Some(id) = frame.get("frame").and_then(Value::as_u64) {
                    frames.insert(id, frame.clone());
                }
            }
        }
    }
    let first_frame = metadata
        .get("first_frame")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("screensaver profile run {run} has no first frame"))?;
    let last_frame = metadata
        .get("last_frame")
        .and_then(Value::as_u64)
        .filter(|last| *last >= first_frame)
        .ok_or_else(|| format!("screensaver profile run {run} has no valid last frame"))?;
    let mut screensaver_frames = Vec::new();
    for frame_id in first_frame..=last_frame {
        let frame = frames
            .get(&frame_id)
            .ok_or_else(|| format!("screensaver profile run {run} is missing frame {frame_id}"))?;
        validate_screensaver_frame_evidence(run, frame_id, frame)?;
        if frame.get("screensaver_active").and_then(Value::as_bool) != Some(true) {
            return Err(format!(
                "screensaver profile run {run} frame {frame_id} is not an active screensaver frame"
            )
            .into());
        }
        screensaver_frames.push(frame);
    }
    if screensaver_frames.len() <= SCREENSAVER_STARTUP_WARMUP_FRAMES {
        return Err(format!(
            "screensaver profile run {run} has no steady-state screensaver frame telemetry"
        )
        .into());
    }
    let (startup, steady) = screensaver_frames.split_at(SCREENSAVER_STARTUP_WARMUP_FRAMES);
    let mut wall = steady
        .iter()
        .map(|frame| frame_u64(frame, "wall_us"))
        .collect::<Vec<_>>();
    let mut work = steady
        .iter()
        .map(|frame| frame_work_us(frame))
        .collect::<Vec<_>>();
    let mut refresh_periods = steady
        .iter()
        .filter_map(|frame| frame.get("vsync_period_us").and_then(Value::as_u64))
        .filter(|value| *value > 0)
        .collect::<Vec<_>>();
    refresh_periods.sort_unstable();
    let refresh_period_us = median_u64(&refresh_periods)
        .ok_or_else(|| format!("screensaver profile run {run} has no refresh period evidence"))?;
    let vsync_misses = steady
        .iter()
        .filter(|frame| {
            frame
                .get("vsync_miss_streak")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        })
        .count();
    let mut presentation_failures = Vec::new();
    for pair_index in SCREENSAVER_STARTUP_WARMUP_FRAMES.saturating_sub(1)
        ..screensaver_frames.len().saturating_sub(1)
    {
        let previous = screensaver_frames[pair_index];
        let current = screensaver_frames[pair_index + 1];
        let frame_id = current.get("frame").and_then(Value::as_u64).unwrap_or(0);
        let previous_sequence = frame_u16(previous, "main_present_sequence");
        let current_sequence = frame_u16(current, "main_present_sequence");
        if !presentation_sequence_is_contiguous(previous_sequence, current_sequence) {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "sequence-gap",
                "previous": previous_sequence,
                "actual": current_sequence,
            }));
        }
        if frame_u16(current, "main_present_active_sequence") != current_sequence
            || current.get("main_present_pending").and_then(Value::as_bool) != Some(false)
        {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "latch-not-complete",
                "posted": current_sequence,
                "active": frame_u16(current, "main_present_active_sequence"),
                "pending": current.get("main_present_pending").cloned().unwrap_or(Value::Null),
            }));
        }
        let flip_delta = frame_u16(current, "main_present_flip_count")
            .wrapping_sub(frame_u16(previous, "main_present_flip_count"));
        if flip_delta != 1 {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "latch-flip-count",
                "delta": flip_delta,
            }));
        }
        let drop_delta = frame_u16(current, "main_present_drop_count")
            .wrapping_sub(frame_u16(previous, "main_present_drop_count"));
        if drop_delta > 0 {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "latch-drop",
                "delta": drop_delta,
            }));
        }
        if current.get("main_present_status").and_then(Value::as_str) != Some("ok") {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "present-status",
                "status": current.get("main_present_status").cloned().unwrap_or(Value::Null),
            }));
        }
        if current
            .get("vsync_miss_streak")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "vsync-miss",
            }));
        }
        if current.get("vsync_source").and_then(Value::as_str) != Some("vsync") {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "vsync-source",
                "source": current.get("vsync_source").cloned().unwrap_or(Value::Null),
            }));
        }
        let previous_render_sequence = frame_u64(previous, "screensaver_render_ahead_sequence");
        let current_render_sequence = frame_u64(current, "screensaver_render_ahead_sequence");
        if current_render_sequence != previous_render_sequence.saturating_add(1) {
            presentation_failures.push(json!({
                "frame": frame_id,
                "kind": "render-sequence-gap",
                "previous": previous_render_sequence,
                "actual": current_render_sequence,
            }));
        }
    }
    let over_budget_frames = steady
        .iter()
        .filter(|frame| {
            frame
                .get("wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
                > refresh_period_us
        })
        .count();
    let startup_max_wall_us = startup
        .iter()
        .filter_map(|frame| frame.get("wall_us").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    let startup_over_budget_frames = startup
        .iter()
        .filter(|frame| {
            frame
                .get("wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
                > refresh_period_us
        })
        .count();
    let outliers = steady
        .iter()
        .filter(|frame| frame_u64(frame, "wall_us") > refresh_period_us)
        .map(|frame| {
            json!({
                "frame": frame_u64(frame, "frame"),
                "wall_us": frame_u64(frame, "wall_us"),
                "prepare_us": frame_u64(frame, "prepare_us"),
                "render_us": frame_u64(frame, "render_us"),
                "present_us": frame_u64(frame, "present_us"),
                "work_us": frame_work_us(frame),
                "process_cpu_us": frame_u64(frame, "process_cpu_us"),
                "runtime_status_write_us": frame_u64(frame, "runtime_status_write_us"),
                "clock_update_us": frame_u64(frame, "clock_update_us"),
                "status_write_due": frame.get("status_write_due").and_then(Value::as_bool).unwrap_or(false),
                "clock_update_due": frame.get("clock_update_due").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    wall.sort_unstable();
    work.sort_unstable();
    let profile_duration_secs = metadata
        .get("duration_secs")
        .and_then(Value::as_f64)
        .filter(|duration| *duration > 0.0)
        .ok_or_else(|| format!("screensaver profile run {run} has no valid duration"))?;
    let submitted_fps = steady.len() as f64 / profile_duration_secs;
    let physical_refresh = physical_refresh_summary(run, steady, refresh_period_us)?;
    let work_signal = steady
        .iter()
        .map(|frame| frame_work_us(frame) as f64)
        .collect::<Vec<_>>();
    let interval_signal = steady
        .windows(2)
        .map(|pair| {
            frame_u64(pair[1], "completion_monotonic_us")
                .saturating_sub(frame_u64(pair[0], "completion_monotonic_us")) as f64
        })
        .collect::<Vec<_>>();
    let periodic = json!({
        "work": periodic_signal(&work_signal, refresh_period_us),
        "presentation_interval": periodic_signal(&interval_signal, refresh_period_us),
    });
    let raster = raster_cadence_summary(steady);
    let render_ahead = screensaver_render_ahead_summary(steady);
    let maintenance = maintenance_cohorts(steady);
    let status_publishing = status_publishing_summary(steady, &active);
    let presentation_paths = steady
        .iter()
        .filter_map(|frame| frame.get("main_present_copy_path").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let steady_present_bytes = steady
        .iter()
        .map(|frame| frame_u64(frame, "present_bytes"))
        .collect::<Vec<_>>();
    let phase_bank_bytes = steady
        .iter()
        .filter_map(|frame| {
            frame
                .get("screensaver_phase_bank_bytes")
                .and_then(Value::as_u64)
        })
        .max()
        .unwrap_or(0);
    let launcher_rss_kb = active
        .iter()
        .filter_map(|sample| {
            sample
                .pointer("/processes/mister-magik-fb/rss_kb")
                .and_then(Value::as_u64)
        })
        .collect::<Vec<_>>();
    let phase_means = json!({
        "prepare_us": mean_frame_field(steady, "prepare_us"),
        "render_us": mean_frame_field(steady, "render_us"),
        "present_us": mean_frame_field(steady, "present_us"),
        "cpu_prepare_us": mean_frame_field(steady, "cpu_prepare_us"),
        "cpu_render_us": mean_frame_field(steady, "cpu_render_us"),
        "cpu_custom_draw_us": mean_frame_field(steady, "cpu_custom_draw_us"),
        "cpu_vsync_us": mean_frame_field(steady, "cpu_vsync_us"),
        "cpu_frame_tail_us": mean_frame_field(steady, "cpu_frame_tail_us"),
        "process_cpu_us": mean_frame_field(steady, "process_cpu_us"),
    });
    let populated_window = populated_screensaver_window(steady, refresh_period_us);
    let device_cpu_samples = active
        .iter()
        .filter_map(|sample| {
            sample
                .pointer("/cpu/combined_busy_pct")
                .and_then(Value::as_f64)
        })
        .collect::<Vec<_>>();
    let core_cpu_samples = |id: u64| {
        active
            .iter()
            .filter_map(|sample| sample.pointer("/cpu/cores").and_then(Value::as_array))
            .flatten()
            .filter(|core| core.get("id").and_then(Value::as_u64) == Some(id))
            .filter_map(|core| core.get("busy_pct").and_then(Value::as_f64))
            .collect::<Vec<_>>()
    };
    let core0_cpu_samples = core_cpu_samples(0);
    let core1_cpu_samples = core_cpu_samples(1);
    let present_errors = presentation_failures
        .iter()
        .filter(|failure| failure.get("kind").and_then(Value::as_str) == Some("present-status"))
        .count();
    let latch_drop_delta = presentation_failures
        .iter()
        .filter(|failure| failure.get("kind").and_then(Value::as_str) == Some("latch-drop"))
        .map(|failure| failure.get("delta").and_then(Value::as_u64).unwrap_or(0))
        .sum::<u64>();
    let mut result = json!({
        "run": run,
        "captured_frames": frames.len(),
        "screensaver_frames": screensaver_frames.len(),
        "measurement_window": {
            "first_frame": first_frame,
            "last_frame": last_frame,
            "frames": screensaver_frames.len(),
        },
        "startup": {
            "ignored_frames": startup.len(),
            "max_wall_us": startup_max_wall_us,
            "over_budget_frames": startup_over_budget_frames,
            "gated": false,
        },
        "steady_state": {
            "frames": steady.len(),
            "submitted_fps": submitted_fps,
            "average_fps": submitted_fps,
            "p99_wall_us": percentile_99(&wall),
            "max_wall_us": wall.last().copied().unwrap_or(0),
            "p99_work_us": percentile_99(&work),
            "refresh_period_us": refresh_period_us,
            "over_budget_frames": over_budget_frames,
            "vsync_misses": vsync_misses,
            "presentation_failures": presentation_failures,
            "physical_refresh": physical_refresh,
        },
        "present_errors": present_errors,
        "latch_drop_delta": latch_drop_delta,
        "outliers": outliers,
        "periodic_timing": periodic,
        "raster_cadence": raster,
        "render_ahead": render_ahead,
        "status_publishing": status_publishing,
        "main_present_copy_paths": presentation_paths,
        "steady_state_present_bytes": {
            "total": steady_present_bytes.iter().sum::<u64>(),
            "max": steady_present_bytes.iter().copied().max().unwrap_or(0),
        },
        "phase_bank_resident_bytes": phase_bank_bytes,
        "launcher_rss": {
            "mean_kb": mean_u64(&launcher_rss_kb),
            "max_kb": launcher_rss_kb.iter().copied().max().unwrap_or(0),
        },
        "maintenance": maintenance,
        "phase_means": phase_means,
        "populated_window": populated_window,
        "cpu_utilization": {
            "launcher_process_pct_of_one_core": mean_frame_field(steady, "process_cpu_us")
                * 100.0 / refresh_period_us as f64,
            "renderer_pct_of_one_core": mean_frame_field(
                steady,
                "screensaver_render_ahead_render_cpu_us"
            ) * 100.0 / refresh_period_us as f64,
            "instrumented_device_combined_busy_pct": mean_f64(&device_cpu_samples),
            "instrumented_device_core0_busy_pct": mean_f64(&core0_cpu_samples),
            "instrumented_device_core1_busy_pct": mean_f64(&core1_cpu_samples),
            "device_scope": "includes Main, launcher, profiler, telemetry agent, kernel, and other processes",
        },
        "profile": metadata,
    });
    let qualification_failures = screensaver_qualification_failures(&result);
    result["qualification"] = json!({
        "qualified": qualification_failures.is_empty(),
        "failures": qualification_failures,
    });
    Ok(result)
}

fn populated_screensaver_window(frames: &[&Value], refresh_period_us: u64) -> Value {
    let last_completion_us = frames
        .last()
        .map(|frame| frame_u64(frame, "completion_monotonic_us"))
        .unwrap_or(0);
    let cutoff_us =
        last_completion_us.saturating_sub(SCREENSAVER_POPULATED_WINDOW_SECS * 1_000_000);
    let populated = frames
        .iter()
        .copied()
        .filter(|frame| frame_u64(frame, "completion_monotonic_us") >= cutoff_us)
        .collect::<Vec<_>>();
    let mut wall = populated
        .iter()
        .map(|frame| frame_u64(frame, "wall_us"))
        .collect::<Vec<_>>();
    let mut work = populated
        .iter()
        .map(|frame| frame_work_us(frame))
        .collect::<Vec<_>>();
    wall.sort_unstable();
    work.sort_unstable();
    let measured_duration_us = populated
        .first()
        .zip(populated.last())
        .map(|(first, last)| {
            frame_u64(last, "completion_monotonic_us")
                .saturating_sub(frame_u64(first, "completion_monotonic_us"))
                .saturating_add(refresh_period_us)
        })
        .unwrap_or(0);
    json!({
        "target_duration_secs": SCREENSAVER_POPULATED_WINDOW_SECS,
        "frames": populated.len(),
        "measured_duration_us": measured_duration_us,
        "average_fps": if measured_duration_us == 0 {
            0.0
        } else {
            populated.len() as f64 * 1_000_000.0 / measured_duration_us as f64
        },
        "p99_wall_us": percentile_99(&wall),
        "max_wall_us": wall.last().copied().unwrap_or(0),
        "p99_work_us": percentile_99(&work),
        "phase_means": {
            "prepare_us": mean_frame_field(&populated, "prepare_us"),
            "render_us": mean_frame_field(&populated, "render_us"),
            "present_us": mean_frame_field(&populated, "present_us"),
            "cpu_custom_draw_us": mean_frame_field(&populated, "cpu_custom_draw_us"),
            "process_cpu_us": mean_frame_field(&populated, "process_cpu_us"),
        },
        "render_ahead": screensaver_render_ahead_summary(&populated),
        "cpu_utilization": {
            "launcher_process_pct_of_one_core": mean_frame_field(
                &populated,
                "process_cpu_us"
            ) * 100.0 / refresh_period_us as f64,
            "renderer_pct_of_one_core": mean_frame_field(
                &populated,
                "screensaver_render_ahead_render_cpu_us"
            ) * 100.0 / refresh_period_us as f64,
        },
    })
}

fn frame_u64(frame: &Value, key: &str) -> u64 {
    frame.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn frame_u16(frame: &Value, key: &str) -> u16 {
    u16::try_from(frame_u64(frame, key)).unwrap_or(0)
}

fn presentation_sequence_is_contiguous(previous: u16, current: u16) -> bool {
    current
        == if previous == u16::MAX {
            1
        } else {
            previous + 1
        }
}

fn median_u64(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        values[middle - 1].saturating_add(values[middle]) / 2
    } else {
        values[middle]
    })
}

fn mean_u64(values: &[u64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
    }
}

fn mean_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn physical_refresh_summary(
    run: usize,
    frames: &[&Value],
    refresh_period_us: u64,
) -> Result<Value> {
    if frames.len() < 2 {
        return Err(format!(
            "screensaver profile run {run} has insufficient physical refresh evidence"
        )
        .into());
    }
    let completions = frames
        .iter()
        .map(|frame| {
            frame
                .get("completion_monotonic_us")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    format!(
                        "screensaver profile run {run} frame {} has no completion timestamp",
                        frame_u64(frame, "frame")
                    )
                })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let elapsed_us = completions
        .last()
        .copied()
        .unwrap_or(0)
        .checked_sub(completions[0])
        .filter(|elapsed| *elapsed > 0)
        .ok_or_else(|| {
            format!("screensaver profile run {run} has non-increasing completion timestamps")
        })?;
    let intervals = completions
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            pair[1]
                .checked_sub(pair[0])
                .map(|interval| (frame_u64(frames[index + 1], "frame"), interval))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            format!("screensaver profile run {run} has non-monotonic completion timestamps")
        })?;
    let latch_flip_deltas = frames
        .windows(2)
        .map(|pair| {
            frame_u16(pair[1], "main_present_flip_count")
                .wrapping_sub(frame_u16(pair[0], "main_present_flip_count")) as u64
        })
        .collect::<Vec<_>>();
    let expected_intervals = intervals
        .iter()
        .map(|(_, interval)| {
            interval
                .saturating_add(refresh_period_us / 2)
                .checked_div(refresh_period_us)
                .unwrap_or(0)
                .max(1)
        })
        .collect::<Vec<_>>();
    let unique_latch_flips = latch_flip_deltas.iter().sum::<u64>();
    let expected_refresh_intervals = expected_intervals.iter().sum::<u64>();
    let repeated_refreshes = expected_intervals
        .iter()
        .zip(&latch_flip_deltas)
        .map(|(expected, flips)| expected.saturating_sub(*flips))
        .sum::<u64>();
    let completion_gap_limit_us = refresh_period_us.saturating_mul(3) / 2;
    let long_completion_intervals = intervals
        .iter()
        .filter(|(_, interval)| *interval > completion_gap_limit_us)
        .map(|(frame, interval)| json!({"frame": frame, "interval_us": interval}))
        .collect::<Vec<_>>();
    Ok(json!({
        "refresh_period_us": refresh_period_us,
        "refresh_hz": 1_000_000.0 / refresh_period_us as f64,
        "elapsed_us": elapsed_us,
        "expected_refresh_intervals": expected_refresh_intervals,
        "unique_latch_flips": unique_latch_flips,
        "repeated_refreshes": repeated_refreshes,
        "unique_fps": unique_latch_flips as f64 * 1_000_000.0 / elapsed_us as f64,
        "max_completion_interval_us": intervals
            .iter()
            .map(|(_, interval)| *interval)
            .max()
            .unwrap_or(0),
        "completion_gap_limit_us": completion_gap_limit_us,
        "long_completion_intervals": long_completion_intervals,
    }))
}

fn frame_work_us(frame: &Value) -> u64 {
    [
        "prepare_us",
        "render_us",
        "custom_draw_us",
        "present_us",
        "runtime_status_write_us",
    ]
    .iter()
    .map(|key| frame_u64(frame, key))
    .sum()
}

fn status_publishing_summary(frames: &[&Value], samples: &[&Value]) -> Value {
    let mode = frames
        .iter()
        .find_map(|frame| frame.get("status_publish_mode").and_then(Value::as_str))
        .or_else(|| {
            samples.iter().find_map(|sample| {
                sample
                    .pointer("/launcher/status_publish_mode")
                    .and_then(Value::as_str)
            })
        })
        .unwrap_or("sync");
    let mut enqueue = frames
        .iter()
        .map(|frame| frame_u64(frame, "status_enqueue_us"))
        .collect::<Vec<_>>();
    let mut worker = frames
        .iter()
        .map(|frame| frame_u64(frame, "status_worker_write_us"))
        .collect::<Vec<_>>();
    let mut synchronous = frames
        .iter()
        .map(|frame| frame_u64(frame, "runtime_status_write_us"))
        .collect::<Vec<_>>();
    enqueue.sort_unstable();
    worker.sort_unstable();
    synchronous.sort_unstable();
    json!({
        "mode": mode,
        "enqueue_p99_us": percentile_99(&enqueue),
        "worker_write_p99_us": percentile_99(&worker),
        "synchronous_write_p99_us": percentile_99(&synchronous),
        "replacement_count": samples
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/status_replaced_count")
                    .and_then(Value::as_u64)
            })
            .max()
            .or_else(|| {
                frames
                    .iter()
                    .filter_map(|frame| {
                        frame.get("status_replaced_count").and_then(Value::as_u64)
                    })
                    .max()
            })
            .unwrap_or(0),
        "final_submitted_sequence": samples
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/status_submitted_sequence")
                    .and_then(Value::as_u64)
            })
            .max()
            .or_else(|| {
                frames
                    .iter()
                    .filter_map(|frame| {
                        frame
                            .get("status_submitted_sequence")
                            .and_then(Value::as_u64)
                    })
                    .max()
            })
            .unwrap_or(0),
        "final_written_sequence": samples
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/status_written_sequence")
                    .and_then(Value::as_u64)
            })
            .max()
            .or_else(|| {
                frames
                    .iter()
                    .filter_map(|frame| {
                        frame
                            .get("status_written_sequence")
                            .and_then(Value::as_u64)
                    })
                    .max()
            })
            .unwrap_or(0),
        "worker_errors": samples
            .iter()
            .filter_map(|sample| {
                sample
                    .pointer("/launcher/status_worker_errors")
                    .and_then(Value::as_u64)
            })
            .max()
            .or_else(|| {
                frames
                    .iter()
                    .filter_map(|frame| {
                        frame.get("status_worker_errors").and_then(Value::as_u64)
                    })
                    .max()
            })
            .unwrap_or(0),
    })
}

fn validate_screensaver_frame_evidence(run: usize, frame_id: u64, frame: &Value) -> Result<()> {
    const U64_FIELDS: &[&str] = &[
        "frame",
        "wall_us",
        "prepare_us",
        "render_us",
        "custom_draw_us",
        "present_us",
        "cpu_prepare_us",
        "cpu_render_us",
        "cpu_custom_draw_us",
        "cpu_vsync_us",
        "cpu_frame_tail_us",
        "process_cpu_us",
        "completion_monotonic_us",
        "vsync_period_us",
        "vsync_miss_streak",
        "vsync_stale_hits",
        "vsync_wait_start_age_us",
        "vsync_accepted_hit_age_us",
        "main_present_sequence",
        "main_present_active_sequence",
        "main_present_flip_count",
        "main_present_drop_count",
        "runtime_status_write_us",
        "status_enqueue_us",
        "status_worker_write_us",
        "status_replaced_count",
        "status_submitted_sequence",
        "status_written_sequence",
        "status_worker_errors",
        "clock_update_us",
        "screensaver_archive_poll_us",
        "screensaver_card_adopt_us",
        "screensaver_parade_advance_us",
        "screensaver_background_us",
        "screensaver_draw_order_us",
        "screensaver_tile_blit_us",
        "screensaver_raster_held_cards",
        "screensaver_raster_moved_cards",
        "screensaver_raster_hold_layer_mask",
        "screensaver_raster_visible_layer_mask",
        "screensaver_render_ahead_sequence",
        "screensaver_render_ahead_queue_depth",
        "screensaver_render_ahead_frame_age_us",
        "screensaver_render_ahead_render_wall_us",
        "screensaver_render_ahead_render_cpu_us",
        "screensaver_render_ahead_starvation_count",
        "screensaver_render_ahead_superseded_frames",
        "screensaver_render_ahead_reused_frames",
        "particle_count",
        "particle_visible",
        "particle_simulation_us",
        "particle_simulation_cpu_us",
        "particle_projection_us",
        "particle_projection_cpu_us",
        "particle_preparation_wait_us",
        "particle_prepared_frame_age_us",
        "particle_lookahead_mismatch_count",
        "particle_preparation_queue_depth",
        "particle_worker_wake_latency_us",
        "particle_clear_us",
        "particle_clear_cpu_us",
        "particle_raster_us",
        "particle_raster_cpu_us",
        "particle_render_cpu_start",
        "particle_render_cpu_end",
        "particle_voluntary_context_switches",
        "particle_involuntary_context_switches",
        "particle_pmu_cycles",
        "particle_pmu_instructions",
        "particle_pmu_cache_references",
        "particle_pmu_cache_misses",
        "particle_pmu_branch_instructions",
        "particle_pmu_branch_misses",
        "particle_rotation_y_millidegrees",
        "particle_simulation_bytes",
        "particle_renderer_scratch_bytes",
    ];
    const BOOL_FIELDS: &[&str] = &[
        "screensaver_active",
        "main_present_pending",
        "status_write_due",
        "clock_update_due",
        "screensaver_render_ahead_cancelled",
        "particle_pmu_available",
    ];
    const STRING_FIELDS: &[&str] = &[
        "vsync_source",
        "main_present_status",
        "screensaver_sampling_profile",
        "status_publish_mode",
        "screensaver_renderer",
        "particle_preset",
        "particle_phase",
        "particle_simulation_backend",
        "particle_projection_backend",
    ];
    for key in U64_FIELDS {
        if frame.get(*key).and_then(Value::as_u64).is_none() {
            return Err(format!(
                "screensaver profile run {run} frame {frame_id} has invalid {key} evidence"
            )
            .into());
        }
    }
    for key in BOOL_FIELDS {
        if frame.get(*key).and_then(Value::as_bool).is_none() {
            return Err(format!(
                "screensaver profile run {run} frame {frame_id} has invalid {key} evidence"
            )
            .into());
        }
    }
    for key in STRING_FIELDS {
        if frame.get(*key).and_then(Value::as_str).is_none() {
            return Err(format!(
                "screensaver profile run {run} frame {frame_id} has invalid {key} evidence"
            )
            .into());
        }
    }
    Ok(())
}

fn periodic_signal(values: &[f64], refresh_period_us: u64) -> Value {
    const RADIUS: usize = 60;
    if values.len() < RADIUS * 2 + 2 {
        return Value::Null;
    }
    let residual = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let start = index.saturating_sub(RADIUS);
            let end = (index + RADIUS + 1).min(values.len());
            let mean = values[start..end].iter().sum::<f64>() / (end - start) as f64;
            value - mean
        })
        .collect::<Vec<_>>();
    let mut best_period = 45.0_f64;
    let mut best_amplitude = 0.0_f64;
    for step in 0..=120 {
        let period = 45.0 + step as f64 * 0.25;
        let amplitude = fourier_amplitude(&residual, period);
        if amplitude > best_amplitude {
            best_period = period;
            best_amplitude = amplitude;
        }
    }
    let refresh_hz = 1_000_000.0 / refresh_period_us.max(1) as f64;
    json!({
        "detrend_window_frames": RADIUS * 2 + 1,
        "scan_min_period_frames": 45.0,
        "scan_max_period_frames": 75.0,
        "period_frames": best_period,
        "frequency_hz": refresh_hz / best_period,
        "amplitude_us": best_amplitude,
        "peak_to_peak_us": best_amplitude * 2.0,
        "second_harmonic_amplitude_us": fourier_amplitude(&residual, best_period / 2.0),
        "third_harmonic_amplitude_us": fourier_amplitude(&residual, best_period / 3.0),
    })
}

fn fourier_amplitude(values: &[f64], period: f64) -> f64 {
    if values.is_empty() || period <= 0.0 {
        return 0.0;
    }
    let (real, imaginary) =
        values
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(real, imaginary), (index, value)| {
                let angle = std::f64::consts::TAU * index as f64 / period;
                (real + value * angle.cos(), imaginary - value * angle.sin())
            });
    2.0 * real.hypot(imaginary) / values.len() as f64
}

fn maintenance_cohorts(frames: &[&Value]) -> Value {
    let cohort = |clock: bool, status: bool| {
        let selected = frames
            .iter()
            .filter(|frame| {
                frame.get("clock_update_due").and_then(Value::as_bool) == Some(clock)
                    && frame.get("status_write_due").and_then(Value::as_bool) == Some(status)
            })
            .copied()
            .collect::<Vec<_>>();
        json!({
            "frames": selected.len(),
            "mean_wall_us": mean_frame_field(&selected, "wall_us"),
            "mean_work_us": if selected.is_empty() {
                0.0
            } else {
                selected.iter().map(|frame| frame_work_us(frame) as f64).sum::<f64>()
                    / selected.len() as f64
            },
            "mean_clock_cost_us": mean_frame_field(&selected, "clock_update_us"),
            "mean_status_cost_us": mean_frame_field(&selected, "runtime_status_write_us"),
        })
    };
    let matched_delta = |due_key: &str| {
        let mut deltas = Vec::new();
        for index in 1..frames.len().saturating_sub(1) {
            if frames[index].get(due_key).and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let before = frames[index - 1];
            let after = frames[index + 1];
            if before.get(due_key).and_then(Value::as_bool) == Some(false)
                && after.get(due_key).and_then(Value::as_bool) == Some(false)
            {
                let baseline = (frame_work_us(before) + frame_work_us(after)) as f64 / 2.0;
                deltas.push(frame_work_us(frames[index]) as f64 - baseline);
            }
        }
        json!({
            "matched_frames": deltas.len(),
            "mean_work_delta_us": if deltas.is_empty() {
                0.0
            } else {
                deltas.iter().sum::<f64>() / deltas.len() as f64
            },
        })
    };
    json!({
        "cohorts": {
            "neither": cohort(false, false),
            "clock_only": cohort(true, false),
            "status_only": cohort(false, true),
            "both": cohort(true, true),
        },
        "matched_neighbors": {
            "clock_update": matched_delta("clock_update_due"),
            "runtime_status_write": matched_delta("status_write_due"),
        },
    })
}

fn mean_frame_field(frames: &[&Value], key: &str) -> f64 {
    if frames.is_empty() {
        return 0.0;
    }
    frames
        .iter()
        .map(|frame| frame_u64(frame, key) as f64)
        .sum::<f64>()
        / frames.len() as f64
}

fn max_frame_field(frames: &[&Value], key: &str) -> u64 {
    frames
        .iter()
        .map(|frame| frame_u64(frame, key))
        .max()
        .unwrap_or(0)
}

fn sum_frame_field(frames: &[&Value], key: &str) -> u64 {
    frames.iter().map(|frame| frame_u64(frame, key)).sum()
}

fn percentile_99_frame_field(frames: &[&Value], key: &str) -> u64 {
    let mut values = frames
        .iter()
        .map(|frame| frame_u64(frame, key))
        .collect::<Vec<_>>();
    values.sort_unstable();
    percentile_99(&values)
}

fn mean_frame_difference(frames: &[&Value], minuend: &str, subtrahend: &str) -> f64 {
    if frames.is_empty() {
        return 0.0;
    }
    frames
        .iter()
        .map(|frame| frame_u64(frame, minuend).saturating_sub(frame_u64(frame, subtrahend)) as f64)
        .sum::<f64>()
        / frames.len() as f64
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn raster_cadence_summary(frames: &[&Value]) -> Value {
    let mut held_frames = 0_u64;
    let mut held_cards = 0_u64;
    let mut moved_cards = 0_u64;
    let mut layer_held_frames = [0_u64; 5];
    let mut layer_visible_frames = [0_u64; 5];
    let mut profiles = std::collections::BTreeSet::new();
    for frame in frames {
        let held = frame_u64(frame, "screensaver_raster_held_cards");
        held_frames += u64::from(held > 0);
        held_cards += held;
        moved_cards += frame_u64(frame, "screensaver_raster_moved_cards");
        let mask = frame_u64(frame, "screensaver_raster_hold_layer_mask") as u8;
        let visible_mask = frame_u64(frame, "screensaver_raster_visible_layer_mask") as u8;
        for layer in 0..layer_held_frames.len() {
            layer_held_frames[layer] += u64::from(mask & (1 << layer) != 0);
            layer_visible_frames[layer] += u64::from(visible_mask & (1 << layer) != 0);
        }
        if let Some(profile) = frame
            .get("screensaver_sampling_profile")
            .and_then(Value::as_str)
        {
            profiles.insert(profile);
        }
    }
    json!({
        "sampling_profiles": profiles,
        "layer_sampling_profiles": std::array::from_fn::<_, 5, _>(|_| {
            profiles.iter().next().copied().unwrap_or("unknown")
        }),
        "held_frames": held_frames,
        "held_card_events": held_cards,
        "moved_card_events": moved_cards,
        "layer_held_frames": layer_held_frames,
        "layer_visible_frames": layer_visible_frames,
        "layer_hold_rates": std::array::from_fn::<_, 5, _>(|layer| {
            if layer_visible_frames[layer] == 0 {
                0.0
            } else {
                layer_held_frames[layer] as f64 / layer_visible_frames[layer] as f64
            }
        }),
    })
}

fn screensaver_render_ahead_summary(frames: &[&Value]) -> Value {
    let values = |key: &str| {
        frames
            .iter()
            .map(|frame| frame_u64(frame, key))
            .collect::<Vec<_>>()
    };
    let summary = |key: &str| {
        let mut samples = values(key);
        samples.sort_unstable();
        json!({
            "mean": mean_u64(&samples),
            "p99": percentile_99(&samples),
            "max": samples.last().copied().unwrap_or(0),
        })
    };
    let counter_delta = |key: &str| {
        frames
            .last()
            .map(|frame| frame_u64(frame, key))
            .unwrap_or(0)
            .saturating_sub(
                frames
                    .first()
                    .map(|frame| frame_u64(frame, key))
                    .unwrap_or(0),
            )
    };
    json!({
        "queue_depth": summary("screensaver_render_ahead_queue_depth"),
        "frame_age_us": summary("screensaver_render_ahead_frame_age_us"),
        "render_wall_us": summary("screensaver_render_ahead_render_wall_us"),
        "render_cpu_us": summary("screensaver_render_ahead_render_cpu_us"),
        "final_sequence": frames.last().map(|frame| frame_u64(frame, "screensaver_render_ahead_sequence")).unwrap_or(0),
        "starvation_count": counter_delta("screensaver_render_ahead_starvation_count"),
        "superseded_frames": counter_delta("screensaver_render_ahead_superseded_frames"),
        "reused_frames": counter_delta("screensaver_render_ahead_reused_frames"),
        "cancelled_frames": frames.iter().filter(|frame| frame.get("screensaver_render_ahead_cancelled").and_then(Value::as_bool) == Some(true)).count(),
    })
}

fn screensaver_qualification_failures(run: &Value) -> Vec<Value> {
    let mut failures = Vec::new();
    let presentation_failures = run
        .pointer("/steady_state/presentation_failures")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if presentation_failures > 0 {
        failures.push(json!({
            "kind": "presentation-failures",
            "count": presentation_failures,
        }));
    }
    let repeated_refreshes = run
        .pointer("/steady_state/physical_refresh/repeated_refreshes")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    if repeated_refreshes > 0 {
        failures.push(json!({
            "kind": "repeated-refreshes",
            "count": repeated_refreshes,
        }));
    }
    let long_completion_intervals = run
        .pointer("/steady_state/physical_refresh/long_completion_intervals")
        .and_then(Value::as_array)
        .map_or(usize::MAX, Vec::len);
    if long_completion_intervals > 0 {
        failures.push(json!({
            "kind": "long-completion-intervals",
            "count": long_completion_intervals,
        }));
    }
    for (kind, pointer) in [
        ("render-ahead-starvation", "/render_ahead/starvation_count"),
        ("render-ahead-reuse", "/render_ahead/reused_frames"),
    ] {
        let count = run
            .pointer(pointer)
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        if count > 0 {
            failures.push(json!({"kind": kind, "count": count}));
        }
    }
    let unique_fps = run
        .pointer("/steady_state/physical_refresh/unique_fps")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let refresh_hz = run
        .pointer("/steady_state/physical_refresh/refresh_hz")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    if unique_fps + 0.1 < refresh_hz {
        failures.push(json!({
            "kind": "unique-fps-below-refresh",
            "unique_fps": unique_fps,
            "refresh_hz": refresh_hz,
            "tolerance_fps": 0.1,
        }));
    }
    let worker_p99_us = run
        .pointer("/populated_window/render_ahead/render_wall_us/p99")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let refresh_period_us = run
        .pointer("/steady_state/refresh_period_us")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if worker_p99_us > refresh_period_us {
        failures.push(json!({
            "kind": "worker-p99-over-refresh-period",
            "worker_p99_us": worker_p99_us,
            "refresh_period_us": refresh_period_us,
        }));
    }
    let copy_paths = run
        .get("main_present_copy_paths")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if copy_paths
        .iter()
        .any(|path| path.as_str() == Some("external-direct"))
    {
        if copy_paths.len() != 1 {
            failures.push(json!({
                "kind": "direct-copy-path-mixed",
                "paths": copy_paths,
            }));
        }
        let max_present_bytes = run
            .pointer("/steady_state_present_bytes/max")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX);
        if max_present_bytes != 0 {
            failures.push(json!({
                "kind": "direct-hidden-copy-bytes",
                "max": max_present_bytes,
            }));
        }
    }
    failures
}

fn screensaver_benchmark_report(summary: &Value) -> Result<String> {
    use std::fmt::Write as _;

    let mut report = String::new();
    writeln!(report, "# 720p Screensaver Benchmark\n")?;
    writeln!(
        report,
        "Installed revision: `{}`  ",
        summary
            .pointer("/manifest/magik_revision")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )?;
    writeln!(
        report,
        "Display: `{}` (retained)\n",
        summary
            .pointer("/display/final_mode")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )?;
    writeln!(
        report,
        "| Run | Profile frames | Submitted FPS | Unique FPS | Repeats | Long gaps | Presentation failures | P99 work | Max wall |"
    )?;
    writeln!(report, "|---:|---:|---:|---:|---:|---:|---:|---:|---:|")?;
    let runs = summary
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("benchmark report has no runs")?;
    for run in runs {
        writeln!(
            report,
            "| {} | {} | {:.2} | {:.2} | {} | {} | {} | {} us | {} us |",
            frame_u64(run, "run"),
            run.pointer("/measurement_window/frames")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/steady_state/submitted_fps")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            run.pointer("/steady_state/physical_refresh/unique_fps")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            run.pointer("/steady_state/physical_refresh/repeated_refreshes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/steady_state/physical_refresh/long_completion_intervals")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            run.pointer("/steady_state/presentation_failures")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            run.pointer("/steady_state/p99_work_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/steady_state/max_wall_us")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )?;
    }
    for run in runs {
        let run_id = frame_u64(run, "run");
        writeln!(report, "\n## Run {run_id}\n")?;
        for (label, pointer) in [
            ("work", "/periodic_timing/work"),
            (
                "presentation interval",
                "/periodic_timing/presentation_interval",
            ),
        ] {
            if let Some(periodic) = run.pointer(pointer).filter(|value| !value.is_null()) {
                writeln!(
                    report,
                    "Strongest 45-75 frame {label} component: {:.2} frames ({:.3} Hz), {:.3} ms amplitude; harmonics {:.3}/{:.3} ms.\n",
                    periodic
                        .get("period_frames")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                    periodic
                        .get("frequency_hz")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                    periodic
                        .get("amplitude_us")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        / 1_000.0,
                    periodic
                        .get("second_harmonic_amplitude_us")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        / 1_000.0,
                    periodic
                        .get("third_harmonic_amplitude_us")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        / 1_000.0,
                )?;
            }
        }
        writeln!(
            report,
            "Physical latch completion: `{}`.\n",
            run.pointer("/steady_state/physical_refresh")
                .cloned()
                .unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Populated final {} seconds: `{}`.\n",
            SCREENSAVER_POPULATED_WINDOW_SECS,
            run.get("populated_window").cloned().unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Status publishing: `{}`. Main present copy paths: `{}`. Phase-bank resident bytes: `{}`. Launcher RSS: `{}`.\n",
            run.get("status_publishing").cloned().unwrap_or(Value::Null),
            run.get("main_present_copy_paths")
                .cloned()
                .unwrap_or(Value::Null),
            run.get("phase_bank_resident_bytes")
                .cloned()
                .unwrap_or(Value::Null),
            run.get("launcher_rss").cloned().unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Visible raster holds: {} frames, {} held-card events, {} moved-card events. Per-layer held/visible/rate: `{}` / `{}` / `{}`. Sampling: `{}`.\n",
            run.pointer("/raster_cadence/held_frames")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/raster_cadence/held_card_events")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/raster_cadence/moved_card_events")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            run.pointer("/raster_cadence/layer_held_frames")
                .cloned()
                .unwrap_or(Value::Null),
            run.pointer("/raster_cadence/layer_visible_frames")
                .cloned()
                .unwrap_or(Value::Null),
            run.pointer("/raster_cadence/layer_hold_rates")
                .cloned()
                .unwrap_or(Value::Null),
            run.pointer("/raster_cadence/sampling_profiles")
                .cloned()
                .unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Maintenance cohorts (neither/clock/status/both): `{}`. Neighbor-matched work deltas: `{}`.\n",
            run.pointer("/maintenance/cohorts")
                .cloned()
                .unwrap_or(Value::Null),
            run.pointer("/maintenance/matched_neighbors")
                .cloned()
                .unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Mean wall phases and process CPU samples: `{}`.\n",
            run.get("phase_means").cloned().unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Normalized CPU utilization: `{}`.\n",
            run.get("cpu_utilization").cloned().unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Render-ahead pipeline: `{}`.\n",
            run.get("render_ahead").cloned().unwrap_or(Value::Null),
        )?;
        writeln!(
            report,
            "Qualification: `{}`.\n",
            run.get("qualification").cloned().unwrap_or(Value::Null),
        )?;
        let failures = run
            .pointer("/steady_state/presentation_failures")
            .cloned()
            .unwrap_or_else(|| json!([]));
        writeln!(report, "Presentation failures: `{failures}`.\n")?;
        let outliers = run
            .get("outliers")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        writeln!(report, "Timing outliers: {}.\n", outliers.len())?;
        if !outliers.is_empty() {
            writeln!(
                report,
                "| Frame | Wall | Work | Render | Present | Process CPU | Status write | Clock update |"
            )?;
            writeln!(report, "|---:|---:|---:|---:|---:|---:|---:|---:|")?;
            for frame in outliers {
                writeln!(
                    report,
                    "| {} | {} us | {} us | {} us | {} us | {} us | {} us | {} us |",
                    frame_u64(frame, "frame"),
                    frame_u64(frame, "wall_us"),
                    frame_u64(frame, "work_us"),
                    frame_u64(frame, "render_us"),
                    frame_u64(frame, "present_us"),
                    frame_u64(frame, "process_cpu_us"),
                    frame_u64(frame, "runtime_status_write_us"),
                    frame_u64(frame, "clock_update_us"),
                )?;
            }
        }
    }
    Ok(report)
}

fn percentile_99(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values[(values.len() * 99).div_ceil(100).saturating_sub(1)]
}

fn action_uses_device(action: &str) -> bool {
    !matches!(
        action,
        "mame-metadata-build"
            | "arcade-database-import"
            | "profile-summary"
            | "-h"
            | "--help"
            | "platform-deploy"
            | "platform-deliver"
            | "platform-rollback"
            | "platform-commit"
    )
}

const RETIRED_PLATFORM_COMMAND_ERROR: &str =
    "platform deployment is only available through scripts/agent deliver";

fn reject_retired_platform_command(action: &str) -> Result<()> {
    if is_retired_platform_command(action) {
        Err(RETIRED_PLATFORM_COMMAND_ERROR.into())
    } else {
        Ok(())
    }
}

fn is_retired_platform_command(action: &str) -> bool {
    matches!(
        action,
        "platform-deploy" | "platform-deliver" | "platform-rollback" | "platform-commit"
    )
}

fn take_reboot_mode_flag(args: &mut Vec<String>) -> Result<RebootMode> {
    let mut flags = Vec::new();
    for flag in [
        "--supervised",
        "--raw",
        "--direct-reset",
        "--direct-reset-no-sync",
    ] {
        if let Some(pos) = args.iter().position(|arg| arg == flag) {
            args.remove(pos);
            flags.push(flag);
        }
    }
    reboot_mode_from_flags(&flags)
}

fn reboot_mode_from_args(args: &[String]) -> Result<RebootMode> {
    let flags: Vec<_> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| {
            matches!(
                *arg,
                "--supervised" | "--raw" | "--direct-reset" | "--direct-reset-no-sync"
            )
        })
        .collect();
    reboot_mode_from_flags(&flags)
}

fn reboot_mode_from_flags(flags: &[&str]) -> Result<RebootMode> {
    if flags.len() > 1 {
        return Err(
            "use only one of --supervised, --raw, --direct-reset, or --direct-reset-no-sync".into(),
        );
    }
    Ok(match flags.first().copied() {
        None | Some("--supervised") => RebootMode::Supervised,
        Some("--raw") => RebootMode::Raw,
        Some("--direct-reset") => RebootMode::DirectReset,
        Some("--direct-reset-no-sync") => RebootMode::DirectResetNoSync,
        Some(other) => return Err(format!("unsupported reboot mode flag: {other}").into()),
    })
}

fn reboot_remote_command(mode: RebootMode) -> String {
    match mode {
        RebootMode::Supervised => acknowledged_main_command("mister_magik_reboot"),
        RebootMode::Raw => RAW_REBOOT_REMOTE_CMD.to_string(),
        RebootMode::DirectReset => DIRECT_RESET_REMOTE_CMD.to_string(),
        RebootMode::DirectResetNoSync => DIRECT_RESET_NO_SYNC_REMOTE_CMD.to_string(),
    }
}

fn issue_reboot(sess: &Session, mode: RebootMode) -> Result<String> {
    if mode.is_direct_reset() {
        eprintln!(
            "WARNING: {} uses Main's direct reset-manager path; it can bypass normal Linux shutdown and previously reproduced Ethernet RX stalls.",
            mode.label()
        );
    }
    let command = reboot_remote_command(mode);
    let out = exec(sess, &command, true)?;
    let mode = out.stdout.trim();
    if mode.is_empty() {
        Ok("unknown".to_string())
    } else {
        Ok(mode.to_string())
    }
}

fn delivery_reboot_mode(running_main: &str, launcher_state: Option<&str>) -> RebootMode {
    if matches!(running_main.trim(), "MiSTer_MagiKDev" | "MiSTer_MagiK")
        && launcher_state == Some("LauncherActive")
    {
        RebootMode::Supervised
    } else {
        RebootMode::Raw
    }
}

fn issue_delivery_reboot(sess: &Session) -> Result<String> {
    let probe = exec(
        sess,
        "if pidof MiSTer_MagiKDev >/dev/null 2>&1; then echo MiSTer_MagiKDev; elif pidof MiSTer_MagiK >/dev/null 2>&1; then echo MiSTer_MagiK; else echo MiSTer; fi",
        true,
    )?;
    if let Some(message) = exec_failure_message("delivery reboot probe", &probe) {
        return Err(message.into());
    }
    let launcher_state = remote_read(sess, MAIN_STATUS_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|status| status.get("launcher_state")?.as_str().map(str::to_owned));
    issue_reboot(
        sess,
        delivery_reboot_mode(&probe.stdout, launcher_state.as_deref()),
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
struct MameMachine {
    setname: String,
    parent_setname: Option<String>,
    title: String,
    year: Option<String>,
    manufacturer: Option<String>,
    sourcefile: Option<String>,
    rotate: Option<i64>,
    display_type: Option<String>,
    display_width: Option<i64>,
    display_height: Option<i64>,
    refresh_hz: Option<f64>,
    players: Option<i64>,
    coins: Option<i64>,
    control_type: Option<String>,
    control_ways: Option<String>,
    buttons: Option<i64>,
    driver_status: Option<String>,
    emulation_status: Option<String>,
    savestate: Option<String>,
    source_version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MameSoftwareItem {
    list_name: String,
    software_name: String,
    parent_name: Option<String>,
    description: String,
    year: Option<String>,
    publisher: Option<String>,
    region: Option<String>,
    source_version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MameSoftwareHash {
    list_name: String,
    software_name: String,
    part_name: Option<String>,
    rom_name: Option<String>,
    size: Option<i64>,
    crc32: Option<String>,
    sha1: Option<String>,
    data_area: Option<String>,
    disk_sha1: Option<String>,
}

fn mame_metadata_build(args: &[String]) -> Result<()> {
    let out = option_value(args, "--out")
        .or_else(|| option_value(args, "-o"))
        .ok_or("mame-metadata-build needs --out <sqlite>")?;
    let machines = if let Some(machine_sqlite) = option_value(args, "--machine-sqlite") {
        load_mame_machines_from_db(Path::new(&machine_sqlite))?
    } else {
        let xml = if let Some(listxml) = option_value(args, "--listxml") {
            fs::read_to_string(listxml)?
        } else {
            let mame = option_value(args, "--mame")
            .or_else(|| env::var("MAME_BIN").ok())
            .or_else(|| find_program_on_path("mame"))
            .ok_or("mame-metadata-build needs --listxml <mame-listxml>, --mame <binary>, MAME_BIN, or mame on PATH")?;
            let output = Command::new(&mame).arg("-listxml").output()?;
            if !output.status.success() {
                return Err(format!(
                    "{mame} -listxml failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            String::from_utf8(output.stdout)?
        };
        parse_mame_listxml(&xml)?
    };
    let (software_items, software_hashes) = load_mame_software_list_xmls(args)?;
    write_mame_metadata_db(
        Path::new(&out),
        &machines,
        &software_items,
        &software_hashes,
    )?;
    println!(
        "mame_metadata_build out={} machines={} software_items={} software_hashes={} source_version={}",
        out,
        machines.len(),
        software_items.len(),
        software_hashes.len(),
        machines
            .first()
            .map(|machine| machine.source_version.as_str())
            .unwrap_or("unknown")
    );
    Ok(())
}

fn load_mame_software_list_xmls(
    args: &[String],
) -> Result<(Vec<MameSoftwareItem>, Vec<MameSoftwareHash>)> {
    const TARGET_LISTS: &[&str] = &["nes", "snes", "n64", "sms", "megadriv", "saturn", "lynx"];

    let mut paths = option_values(args, "--software-list")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(dir) = option_value(args, "--software-dir") {
        let dir = PathBuf::from(dir);
        for list in TARGET_LISTS {
            let path = dir.join(format!("{list}.xml"));
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();

    let mut items = Vec::new();
    let mut hashes = Vec::new();
    for path in paths {
        let xml = fs::read_to_string(&path)?;
        let (mut list_items, mut list_hashes) = parse_mame_software_list_xml(&xml)?;
        items.append(&mut list_items);
        hashes.append(&mut list_hashes);
    }
    Ok((items, hashes))
}

fn parse_mame_listxml(xml: &str) -> Result<Vec<MameMachine>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut machines = Vec::new();
    let mut source_version = "unknown".to_string();
    let mut current: Option<MameMachine> = None;
    let mut field = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "mame" => {
                        if let Some(build) = attr_value(&e, b"build") {
                            source_version = build;
                        }
                    }
                    "machine" => {
                        let setname = attr_value(&e, b"name").unwrap_or_default();
                        current = Some(MameMachine {
                            setname,
                            parent_setname: attr_value(&e, b"cloneof"),
                            sourcefile: attr_value(&e, b"sourcefile"),
                            source_version: source_version.clone(),
                            ..MameMachine::default()
                        });
                    }
                    "description" | "year" | "manufacturer" if current.is_some() => field = tag,
                    "input" => {
                        if let Some(machine) = current.as_mut() {
                            apply_mame_input(machine, &e);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if let Some(machine) = current.as_mut() {
                    match tag.as_str() {
                        "display" if machine.display_type.is_none() => {
                            apply_mame_display(machine, &e)
                        }
                        "input" => apply_mame_input(machine, &e),
                        "control" => apply_mame_control(machine, &e),
                        "driver" => apply_mame_driver(machine, &e),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(machine) = current.as_mut() {
                    let text = e.xml10_content().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "description" => machine.title = text,
                        "year" => machine.year = Some(text),
                        "manufacturer" => machine.manufacturer = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "machine"
                    && let Some(mut machine) = current.take()
                {
                    if machine.title.is_empty() {
                        machine.title = machine.setname.clone();
                    }
                    machines.push(machine);
                }
                field.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("parse MAME listxml: {e}").into()),
            _ => {}
        }
    }
    Ok(machines)
}

fn parse_mame_software_list_xml(
    xml: &str,
) -> Result<(Vec<MameSoftwareItem>, Vec<MameSoftwareHash>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut hashes = Vec::new();
    let mut list_name = String::new();
    let mut source_version = "software-list".to_string();
    let mut current: Option<MameSoftwareItem> = None;
    let mut current_part: Option<String> = None;
    let mut current_data_area: Option<String> = None;
    let mut field = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "softwarelist" => {
                        list_name = attr_value(&e, b"name").unwrap_or_default();
                        if let Some(build) = attr_value(&e, b"build") {
                            source_version = build;
                        }
                    }
                    "software" => {
                        let software_name = attr_value(&e, b"name").unwrap_or_default();
                        current = Some(MameSoftwareItem {
                            list_name: list_name.clone(),
                            software_name,
                            parent_name: attr_value(&e, b"cloneof"),
                            source_version: source_version.clone(),
                            ..MameSoftwareItem::default()
                        });
                    }
                    "description" | "year" | "publisher" if current.is_some() => field = tag,
                    "part" if current.is_some() => current_part = attr_value(&e, b"name"),
                    "dataarea" | "diskarea" if current.is_some() => {
                        current_data_area = attr_value(&e, b"name")
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if let Some(item) = current.as_ref() {
                    match tag.as_str() {
                        "rom" => hashes.push(MameSoftwareHash {
                            list_name: item.list_name.clone(),
                            software_name: item.software_name.clone(),
                            part_name: current_part.clone(),
                            rom_name: attr_value(&e, b"name"),
                            size: attr_value(&e, b"size").and_then(|value| value.parse().ok()),
                            crc32: attr_value(&e, b"crc").map(|value| value.to_ascii_lowercase()),
                            sha1: attr_value(&e, b"sha1").map(|value| value.to_ascii_lowercase()),
                            data_area: current_data_area.clone(),
                            disk_sha1: None,
                        }),
                        "disk" => hashes.push(MameSoftwareHash {
                            list_name: item.list_name.clone(),
                            software_name: item.software_name.clone(),
                            part_name: current_part.clone(),
                            rom_name: attr_value(&e, b"name"),
                            size: None,
                            crc32: None,
                            sha1: None,
                            data_area: current_data_area.clone(),
                            disk_sha1: attr_value(&e, b"sha1")
                                .map(|value| value.to_ascii_lowercase()),
                        }),
                        _ => {}
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(item) = current.as_mut() {
                    let text = e.xml10_content().unwrap_or_default().into_owned();
                    match field.as_str() {
                        "description" => {
                            item.description = text;
                            item.region = region_from_text(&item.description).map(str::to_string);
                        }
                        "year" => item.year = Some(text),
                        "publisher" => item.publisher = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match tag.as_str() {
                    "software" => {
                        if let Some(mut item) = current.take() {
                            if item.description.is_empty() {
                                item.description = item.software_name.clone();
                            }
                            if item.region.is_none() {
                                item.region =
                                    region_from_text(&item.description).map(str::to_string);
                            }
                            items.push(item);
                        }
                        current_part = None;
                        current_data_area = None;
                    }
                    "part" => current_part = None,
                    "dataarea" | "diskarea" => current_data_area = None,
                    "description" | "year" | "publisher" => field.clear(),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("parse software list XML: {e}").into()),
            _ => {}
        }
    }

    Ok((items, hashes))
}

fn load_mame_machines_from_db(path: &Path) -> Result<Vec<MameMachine>> {
    let conn = Connection::open(path)?;
    let sql =
        "SELECT setname,parent_setname,title,year,manufacturer,sourcefile,rotate,display_type,
                display_width,display_height,refresh_hz,players,coins,control_type,control_ways,
                buttons,driver_status,emulation_status,savestate,source_version
         FROM mame_machines
         ORDER BY setname";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(MameMachine {
            setname: row.get(0)?,
            parent_setname: row.get(1)?,
            title: row.get(2)?,
            year: row.get(3)?,
            manufacturer: row.get(4)?,
            sourcefile: row.get(5)?,
            rotate: row.get(6)?,
            display_type: row.get(7)?,
            display_width: row.get(8)?,
            display_height: row.get(9)?,
            refresh_hz: row.get(10)?,
            players: row.get(11)?,
            coins: row.get(12)?,
            control_type: row.get(13)?,
            control_ways: row.get(14)?,
            buttons: row.get(15)?,
            driver_status: row.get(16)?,
            emulation_status: row.get(17)?,
            savestate: row.get(18)?,
            source_version: row.get(19)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn region_from_text(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    if contains_any(
        &lower,
        &["(usa", "(us)", "(u)", "[usa", "[us]", " usa", " ntsc-u"],
    ) {
        Some("usa")
    } else if contains_any(
        &lower,
        &[
            "(europe", "(eu", "(e)", "[europe", "[eu]", " europe", " pal",
        ],
    ) {
        Some("europe")
    } else if contains_any(
        &lower,
        &[
            "(japan", "(jp", "(j)", "[japan", "[jp]", " japan", " ntsc-j",
        ],
    ) {
        Some("japan")
    } else if contains_any(&lower, &["(korea", "[korea", " korea"]) {
        Some("korea")
    } else if contains_any(&lower, &["(world", "(w)", "[world", " world"]) {
        Some("world")
    } else {
        None
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn find_program_on_path(name: &str) -> Option<String> {
    let paths = env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|ext| !ext.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_else(|| vec![".exe".into(), ".bat".into(), ".cmd".into()])
    } else {
        vec![String::new()]
    };
    for dir in env::split_paths(&paths) {
        for extension in &extensions {
            let candidate = if extension.is_empty() || name.ends_with(extension) {
                dir.join(name)
            } else {
                dir.join(format!("{name}{extension}"))
            };
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

fn apply_mame_display(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.display_type = attr_value(e, b"type");
    machine.rotate = attr_value(e, b"rotate").and_then(|value| value.parse().ok());
    machine.display_width = attr_value(e, b"width").and_then(|value| value.parse().ok());
    machine.display_height = attr_value(e, b"height").and_then(|value| value.parse().ok());
    machine.refresh_hz = attr_value(e, b"refresh").and_then(|value| value.parse().ok());
}

fn apply_mame_input(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.players = attr_value(e, b"players").and_then(|value| value.parse().ok());
    machine.coins = attr_value(e, b"coins").and_then(|value| value.parse().ok());
}

fn apply_mame_control(machine: &mut MameMachine, e: &BytesStart<'_>) {
    if machine.control_type.is_none() {
        machine.control_type = attr_value(e, b"type");
    }
    if machine.control_ways.is_none() {
        machine.control_ways = attr_value(e, b"ways");
    }
    if let Some(buttons) = attr_value(e, b"buttons").and_then(|value| value.parse::<i64>().ok()) {
        machine.buttons = Some(machine.buttons.unwrap_or(0).max(buttons));
    }
}

fn apply_mame_driver(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.driver_status = attr_value(e, b"status");
    machine.emulation_status = attr_value(e, b"emulation");
    machine.savestate = attr_value(e, b"savestate");
}

fn attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref() == key)
        .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).into_owned())
}

fn write_mame_metadata_db(
    path: &Path,
    machines: &[MameMachine],
    software_items: &[MameSoftwareItem],
    software_hashes: &[MameSoftwareHash],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("sqlite3.tmp");
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut conn = Connection::open(&tmp)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT,
            sourcefile TEXT,
            rotate INTEGER,
            display_type TEXT,
            display_width INTEGER,
            display_height INTEGER,
            refresh_hz REAL,
            players INTEGER,
            coins INTEGER,
            control_type TEXT,
            control_ways TEXT,
            buttons INTEGER,
            driver_status TEXT,
            emulation_status TEXT,
            savestate TEXT,
            source_version TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_items (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            parent_name TEXT,
            description TEXT NOT NULL,
            year TEXT,
            publisher TEXT,
            region TEXT,
            source_version TEXT NOT NULL,
            PRIMARY KEY(list_name, software_name)
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_hashes (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            part_name TEXT,
            rom_name TEXT,
            size INTEGER,
            crc32 TEXT,
            sha1 TEXT,
            data_area TEXT,
            disk_sha1 TEXT
        );
        CREATE INDEX mame_software_hashes_crc_idx
            ON mame_software_hashes(list_name, size, crc32);
        CREATE INDEX mame_software_hashes_disk_idx
            ON mame_software_hashes(list_name, disk_sha1);
        "#,
    )?;
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_machines(
                setname,parent_setname,title,year,manufacturer,sourcefile,rotate,display_type,
                display_width,display_height,refresh_hz,players,coins,control_type,control_ways,
                buttons,driver_status,emulation_status,savestate,source_version
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        )?;
        for machine in machines {
            stmt.execute(params![
                machine.setname,
                machine.parent_setname,
                machine.title,
                machine.year,
                machine.manufacturer,
                machine.sourcefile,
                machine.rotate,
                machine.display_type,
                machine.display_width,
                machine.display_height,
                machine.refresh_hz,
                machine.players,
                machine.coins,
                machine.control_type,
                machine.control_ways,
                machine.buttons,
                machine.driver_status,
                machine.emulation_status,
                machine.savestate,
                machine.source_version
            ])?;
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_software_items(
                list_name,software_name,parent_name,description,year,publisher,region,source_version
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )?;
        for item in software_items {
            stmt.execute(params![
                item.list_name,
                item.software_name,
                item.parent_name,
                item.description,
                item.year,
                item.publisher,
                item.region,
                item.source_version
            ])?;
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO mame_software_hashes(
                list_name,software_name,part_name,rom_name,size,crc32,sha1,data_area,disk_sha1
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        for hash in software_hashes {
            stmt.execute(params![
                hash.list_name,
                hash.software_name,
                hash.part_name,
                hash.rom_name,
                hash.size,
                hash.crc32,
                hash.sha1,
                hash.data_area,
                hash.disk_sha1
            ])?;
        }
    }
    tx.commit()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn deploy_magik_bundle(
    sess: &Session,
    local: &Path,
    remote: &str,
    manifest_local: &Path,
    manifest_remote: &str,
    expected_sha256: &str,
) -> Result<()> {
    let total_t = Instant::now();
    let validate_t = Instant::now();
    let transaction = MagikDeployTransaction::validate_bundle(
        local,
        remote,
        manifest_local,
        manifest_remote,
        expected_sha256,
    )?;
    let validate_ms = validate_t.elapsed().as_millis();
    let report = transaction.run_ssh(sess, validate_ms, total_t)?;
    report.print();
    Ok(())
}

fn shell_sequence<I, S>(commands: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    commands
        .into_iter()
        .map(|command| {
            let command = command.as_ref().trim();
            assert!(
                !command.is_empty(),
                "shell command fragment must not be empty"
            );
            assert!(
                !command.starts_with(';') && !command.ends_with(';'),
                "shell command fragments must not own sequence separators"
            );
            command.to_string()
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn exec_checked(sess: &Session, label: &str, command: &str) -> Result<()> {
    let output = exec(sess, command, true)?;
    if let Some(message) = exec_failure_message(label, &output) {
        Err(message.into())
    } else {
        Ok(())
    }
}

fn exec_checked_output(sess: &Session, label: &str, command: &str) -> Result<ExecOutput> {
    let output = exec(sess, command, true)?;
    if let Some(message) = exec_failure_message(label, &output) {
        Err(message.into())
    } else {
        Ok(output)
    }
}

fn file_sha256(path: PathBuf) -> Result<String> {
    let mut source = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(encode_hex(&hasher.finalize()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MagikDeployTransaction {
    local: PathBuf,
    remote: String,
    remote_dir: String,
    upload: String,
    lock: String,
    local_bytes: u64,
    expected_sha256: String,
    manifest_sha256: String,
    manifest: ManifestDeploy,
}

const RUNTIME_MANIFEST_FIELDS: &[&str] = &[
    "format",
    "platform_release",
    "platform_release_number",
    "platform_bundle_id",
    "qualification_candidate_id",
    "latch_protocol_version",
    "latch_capability_mask",
    "main_path",
    "gui_path",
    "manager_path",
    "scanout_module_path",
    "scanout_metadata_path",
    "latch_rbf_path",
    "latch_metadata_path",
    "main_sha256",
    "gui_sha256",
    "manager_sha256",
    "scanout_module_sha256",
    "scanout_metadata_sha256",
    "latch_rbf_sha256",
    "latch_metadata_sha256",
    "platform_contract_sha256",
    "main_revision",
    "magik_revision",
    "menu_revision",
];

fn validate_runtime_bundle_identity(
    local: &Path,
    manifest_local: &Path,
    expected_sha256: &str,
) -> Result<String> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("runtime binary expected SHA-256 is not canonical lowercase hex".into());
    }
    let actual_sha256 = file_sha256(local.to_path_buf())?;
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "runtime binary hash mismatch expected={expected_sha256} actual={actual_sha256}"
        )
        .into());
    }
    let text = fs::read_to_string(manifest_local)?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or("runtime manifest contains a malformed line")?;
        if value.is_empty()
            || !RUNTIME_MANIFEST_FIELDS.contains(&key)
            || fields.insert(key, value).is_some()
        {
            return Err(
                format!("runtime manifest has an invalid or duplicate field: {key}").into(),
            );
        }
    }
    if fields.len() != RUNTIME_MANIFEST_FIELDS.len()
        || RUNTIME_MANIFEST_FIELDS
            .iter()
            .any(|field| !fields.contains_key(field))
    {
        return Err("runtime manifest does not have the exact canonical field set".into());
    }
    for (key, expected) in [
        ("format", "mister-magik-platform-v3"),
        ("latch_protocol_version", "4"),
        ("latch_capability_mask", "0x01ff"),
        ("main_path", "/media/fat/MiSTer_MagiKDev"),
        ("gui_path", "/media/fat/mister-magik-dev/mister-magik-fb"),
        (
            "manager_path",
            "/media/fat/mister-magik-dev/mister-magik-manager",
        ),
        (
            "scanout_module_path",
            "/media/fat/mister-magik-dev/mister_magik_scanout_slots.ko",
        ),
        (
            "scanout_metadata_path",
            "/media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt",
        ),
        (
            "latch_rbf_path",
            "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf",
        ),
        (
            "latch_metadata_path",
            "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt",
        ),
        ("gui_sha256", expected_sha256),
    ] {
        if fields.get(key).copied() != Some(expected) {
            return Err(format!("runtime manifest field {key} is not canonical").into());
        }
    }
    file_sha256(manifest_local.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestDeploy {
    local: PathBuf,
    remote: String,
    upload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MagikDeployReport {
    local: PathBuf,
    remote: String,
    local_bytes: u64,
    remote_bytes: u64,
    total_ms: u128,
    validate_ms: u128,
    prepare_ms: u128,
    suspend_ms: u128,
    upload_ms: u128,
    swap_ms: u128,
    chmod_size_ms: u128,
    resume_ms: u128,
    cleanup_ms: u128,
}

impl MagikDeployTransaction {
    fn validate_bundle(
        local: &Path,
        remote: &str,
        manifest_local: &Path,
        manifest_remote: &str,
        expected_sha256: &str,
    ) -> Result<Self> {
        if !remote.starts_with('/') || remote.ends_with('/') || remote.contains('\0') {
            return Err(format!("unsupported deploy remote: {remote}").into());
        }
        if remote.split('/').any(|part| part == "." || part == "..") {
            return Err(format!("unsupported deploy remote path component: {remote}").into());
        }
        if remote != "/media/fat/mister-magik-dev/mister-magik-fb"
            || manifest_remote != "/media/fat/mister-magik-dev/platform-v3.manifest"
            || !local.is_file()
            || !manifest_local.is_file()
        {
            return Err(
                "runtime deployment requires the canonical development binary and manifest bundle"
                    .into(),
            );
        }
        let remote_dir = remote_parent_dir(remote)?.to_string();
        let local_bytes = fs::metadata(local)?.len();
        let manifest_sha256 =
            validate_runtime_bundle_identity(local, manifest_local, expected_sha256)?;
        Ok(Self {
            local: local.to_path_buf(),
            remote: remote.to_string(),
            upload: format!("{remote}.upload"),
            lock: format!("{remote_dir}/deploy.lock"),
            remote_dir,
            local_bytes,
            expected_sha256: expected_sha256.into(),
            manifest_sha256,
            manifest: ManifestDeploy {
                local: manifest_local.to_path_buf(),
                remote: manifest_remote.into(),
                upload: format!("{manifest_remote}.upload"),
            },
        })
    }

    fn run_ssh(
        &self,
        sess: &Session,
        validate_ms: u128,
        total_t: Instant,
    ) -> Result<MagikDeployReport> {
        self.run_with(&SshDeployRemote { sess }, validate_ms, total_t)
    }

    fn run_with<R: DeployRemote>(
        &self,
        remote: &R,
        validate_ms: u128,
        total_t: Instant,
    ) -> Result<MagikDeployReport> {
        let prepare_ms = match self.prepare(remote) {
            Ok(elapsed) => elapsed,
            Err(error) => {
                let _ = self.cleanup(remote);
                return Err(error);
            }
        };
        let mut suspended = false;
        let mut cleaned = false;
        let result = (|| -> Result<MagikDeployReport> {
            let suspend_t = Instant::now();
            deploy_fifo_command(remote, "mister_magik_suspend")?;
            let suspend_ms = suspend_t.elapsed().as_millis();
            suspended = true;

            let upload_t = Instant::now();
            remote.put(&self.local, &self.upload)?;
            remote.put(&self.manifest.local, &self.manifest.upload)?;
            self.verify_uploads(remote)?;
            let upload_ms = upload_t.elapsed().as_millis();

            let swap_ms = self.swap_upload(remote)?;
            let (chmod_size_ms, remote_bytes) = self.chmod_and_verify_size(remote)?;

            let cleanup_ms = self.cleanup(remote)?;
            cleaned = true;

            let resume_t = Instant::now();
            deploy_fifo_command(remote, "mister_magik_resume")?;
            let resume_ms = resume_t.elapsed().as_millis();
            suspended = false;

            Ok(MagikDeployReport {
                local: self.local.clone(),
                remote: self.remote.clone(),
                local_bytes: self.local_bytes,
                remote_bytes,
                total_ms: total_t.elapsed().as_millis(),
                validate_ms,
                prepare_ms,
                suspend_ms,
                upload_ms,
                swap_ms,
                chmod_size_ms,
                resume_ms,
                cleanup_ms,
            })
        })();

        if result.is_err() {
            if !cleaned {
                let _ = self.cleanup(remote);
            }
            if suspended {
                let _ = deploy_fifo_command(remote, "mister_magik_resume");
            }
        }
        result
    }

    fn prepare<R: DeployRemote>(&self, remote: &R) -> Result<u128> {
        let start = Instant::now();
        self.exec_phase(
            remote,
            "prepare",
            &format!("mkdir -p {}; : > {}", sh(&self.remote_dir), sh(&self.lock)),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn swap_upload<R: DeployRemote>(&self, remote: &R) -> Result<u128> {
        let start = Instant::now();
        let manifest_swap = format!(
            "; mv {} {}",
            sh(&self.manifest.upload),
            sh(&self.manifest.remote)
        );
        self.exec_phase(
            remote,
            "swap",
            &format!(
                "set -eu; mv {} {}{manifest_swap}; sync",
                sh(&self.upload),
                sh(&self.remote)
            ),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn verify_uploads<R: DeployRemote>(&self, remote: &R) -> Result<()> {
        self.exec_phase(
            remote,
            "upload-identity",
            &format!(
                "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}",
                sh(&self.upload),
                sh(&self.expected_sha256),
                sh(&self.manifest.upload),
                sh(&self.manifest_sha256),
            ),
        )?;
        Ok(())
    }

    fn chmod_and_verify_size<R: DeployRemote>(&self, remote: &R) -> Result<(u128, u64)> {
        let start = Instant::now();
        let out = self.exec_phase(
            remote,
            "chmod-size-verify",
            &self.chmod_size_verify_command(),
        )?;
        let remote_bytes = parse_wc_byte_count(&out.stdout)
            .ok_or_else(|| format!("unable to parse deployed size from: {}", out.stdout.trim()))?;
        if remote_bytes != self.local_bytes {
            return Err(format!(
                "deployed size mismatch local={} remote={}",
                self.local_bytes, remote_bytes
            )
            .into());
        }
        Ok((start.elapsed().as_millis(), remote_bytes))
    }

    fn chmod_size_verify_command(&self) -> String {
        format!(
            "chmod +x {} && wc -c {}",
            sh(&self.remote),
            sh(&self.remote)
        )
    }

    fn cleanup<R: DeployRemote>(&self, remote: &R) -> Result<u128> {
        let start = Instant::now();
        let manifest_upload = format!(" {}", sh(&self.manifest.upload));
        self.exec_phase(
            remote,
            "cleanup",
            &format!(
                "rm -f {} {}{manifest_upload}",
                sh(&self.upload),
                sh(&self.lock)
            ),
        )?;
        Ok(start.elapsed().as_millis())
    }

    fn exec_phase<R: DeployRemote>(
        &self,
        remote: &R,
        phase: &str,
        command: &str,
    ) -> Result<ExecOutput> {
        let out = remote.exec(command)?;
        if out.rc != 0 {
            return Err(format!(
                "deploy {phase} phase failed rc={} output={}",
                out.rc,
                out.stdout.trim()
            )
            .into());
        }
        Ok(out)
    }
}

trait DeployRemote {
    fn exec(&self, command: &str) -> Result<ExecOutput>;
    fn put(&self, local: &Path, remote: &str) -> Result<()>;
}

struct SshDeployRemote<'a> {
    sess: &'a Session,
}

impl DeployRemote for SshDeployRemote<'_> {
    fn exec(&self, command: &str) -> Result<ExecOutput> {
        exec(self.sess, command, true)
    }

    fn put(&self, local: &Path, remote: &str) -> Result<()> {
        put(self.sess, local, remote)
    }
}

fn deploy_fifo_command<R: DeployRemote>(remote: &R, command: &str) -> Result<()> {
    let out = remote.exec(&acknowledged_main_command(command))?;
    if out.rc == 0 {
        Ok(())
    } else {
        Err(format!("MiSTer command failed: {command}").into())
    }
}

impl MagikDeployReport {
    fn print(&self) {
        let finish_ms = self.swap_ms + self.chmod_size_ms;
        let resume_size_ms = self.resume_ms + self.chmod_size_ms;
        println!(
            "deploy_runtime_bundle local={} remote={} local_bytes={} remote_bytes={} total_ms={} prepare_ms={} suspend_ms={} put_ms={} finish_ms={} resume_size_ms={} validate_ms={} upload_ms={} swap_ms={} chmod_size_ms={} resume_ms={} cleanup_ms={}",
            self.local.display(),
            self.remote,
            self.local_bytes,
            self.remote_bytes,
            self.total_ms,
            self.prepare_ms,
            self.suspend_ms,
            self.upload_ms,
            finish_ms,
            resume_size_ms,
            self.validate_ms,
            self.upload_ms,
            self.swap_ms,
            self.chmod_size_ms,
            self.resume_ms,
            self.cleanup_ms
        );
    }
}

fn parse_wc_byte_count(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse::<u64>().ok()
}

fn append_profile_row(path: &str, header: &str, row: &str) -> Result<()> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let needs_header = !path.exists() || path.metadata()?.len() == 0;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if needs_header {
        writeln!(file, "{header}")?;
    }
    writeln!(file, "{row}")?;
    Ok(())
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn bytes_for_profile(size: usize) -> Vec<u8> {
    let mut x = 0x4d49_5354_4552_4d47u64;
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    bytes.truncate(size);
    bytes
}

fn parse_profile_count(args: &[String], default: usize) -> usize {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--bytes" | "--timeout" | "--probe-timeout-ms" | "--sleep-ms"
        ) {
            skip_value = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if let Ok(samples) = arg.parse::<usize>() {
            return samples;
        }
    }
    default
}

fn parse_profile_bytes(args: &[String], default: usize) -> usize {
    option_value(args, "--bytes")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
}

fn connection_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 5);
    let bytes_len = parse_profile_bytes(args, 4 * 1024 * 1024);
    let out_path = "history/toolchain-bench/results-connection-profile.tsv";
    let header = "kind\tts_unix_ms\tsample\thost\tbytes\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tsftp_init_ms\tput_tmp_ms\tput_tmp_mib_s\tput_fat_ms\tput_fat_mib_s\tuptime\tnote";
    println!("{header}");
    let bytes = bytes_for_profile(bytes_len);
    for sample in 1..=samples {
        let ts = unix_ms_now();
        match connect_timed(10) {
            Ok(timed) => {
                let exec_t = Instant::now();
                let uptime_out = exec(&timed.sess, "cat /proc/uptime", true)?;
                let exec_ms = exec_t.elapsed().as_millis();
                let uptime = uptime_out.stdout.split_whitespace().next().unwrap_or("");
                let sftp_t = Instant::now();
                let _ = timed.sess.sftp()?;
                let sftp_init_ms = sftp_t.elapsed().as_millis();
                let tag = format!("{}-{sample}-{ts}", std::process::id());
                let tmp_remote = format!("/tmp/mister-magik-profile-{tag}.bin");
                let app = configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik");
                let fat_remote = format!("{app}/profile-tmp-{tag}.bin");
                let put_tmp_ms = sftp_write_profile(&timed.sess, &tmp_remote, &bytes)?;
                let _ = exec(
                    &timed.sess,
                    &format!("mkdir -p {} >/dev/null 2>&1 || true", sh(&app)),
                    true,
                );
                let put_fat_ms = sftp_write_profile(&timed.sess, &fat_remote, &bytes)?;
                let _ = exec(
                    &timed.sess,
                    &format!("rm -f {} {}", sh(&tmp_remote), sh(&fat_remote)),
                    true,
                );
                let mib = bytes_len as f64 / (1024.0 * 1024.0);
                let tmp_mib_s = if put_tmp_ms > 0 {
                    mib * 1000.0 / put_tmp_ms as f64
                } else {
                    0.0
                };
                let fat_mib_s = if put_fat_ms > 0 {
                    mib * 1000.0 / put_fat_ms as f64
                } else {
                    0.0
                };
                let row = format!(
                    "connection\t{ts}\t{sample}\t{}\t{bytes_len}\t{}\t{}\t{}\t{}\t{exec_ms}\t{sftp_init_ms}\t{put_tmp_ms}\t{tmp_mib_s:.2}\t{put_fat_ms}\t{fat_mib_s:.2}\t{uptime}\tok",
                    host(),
                    timed.resolve_ms,
                    timed.tcp_ms,
                    timed.handshake_ms,
                    timed.auth_ms
                );
                println!("{row}");
                append_profile_row(out_path, header, &row)?;
            }
            Err(err) => {
                let row = format!(
                    "connection\t{ts}\t{sample}\t{}\t{bytes_len}\t\t\t\t\t\t\t\t\t\t\t\tERROR: {err}",
                    host()
                );
                println!("{row}");
                append_profile_row(out_path, header, &row)?;
            }
        }
        thread::sleep(Duration::from_millis(300));
    }
    eprintln!("connection-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

fn agent_cli(args: &[String]) -> Result<()> {
    let subcommand = args.first().map(String::as_str).unwrap_or("status");
    match subcommand {
        "ping" => {
            let reply = agent_request("ping", json!({}), Duration::from_secs(2))?;
            println!(
                "agent pong after {}ms: {}",
                reply.elapsed_ms,
                serde_json::to_string(reply.response.get("result").unwrap_or(&Value::Null))?
            );
        }
        "status" => {
            let reply = agent_request("status", json!({}), Duration::from_secs(2))?;
            println!(
                "{}",
                serde_json::to_string_pretty(reply.response.get("result").unwrap_or(&Value::Null))?
            );
        }
        "logs" => {
            let json_out = args.iter().any(|arg| arg == "--json");
            let reply = agent_request("logs", json!({}), Duration::from_secs(2))?;
            let result = reply.response.get("result").unwrap_or(&Value::Null);
            if json_out {
                println!("{}", serde_json::to_string_pretty(result)?);
            } else if let Some(lines) = result.get("lines").and_then(Value::as_array) {
                for line in lines.iter().filter_map(Value::as_str) {
                    println!("{line}");
                }
                eprintln!(
                    "agent logs: {} line(s), {} dropped, {}ms",
                    result.get("count").and_then(Value::as_u64).unwrap_or(0),
                    result.get("dropped").and_then(Value::as_u64).unwrap_or(0),
                    reply.elapsed_ms
                );
            } else {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
        }
        "timeline" => {
            let json_out = args.iter().any(|arg| arg == "--json");
            let reply = agent_request("timeline", json!({}), Duration::from_secs(2))?;
            let result = reply.response.get("result").unwrap_or(&Value::Null);
            if json_out {
                println!("{}", serde_json::to_string_pretty(result)?);
            } else if let Some(events) = result.get("events").and_then(Value::as_array) {
                for event in events {
                    let uptime_ms = event.get("uptime_ms").and_then(Value::as_u64).unwrap_or(0);
                    let name = event.get("event").and_then(Value::as_str).unwrap_or("");
                    let detail = event.get("detail").and_then(Value::as_str).unwrap_or("");
                    println!("{uptime_ms}\t{name}\t{detail}");
                }
                eprintln!(
                    "agent timeline: {} event(s), {} dropped, {}ms",
                    result.get("count").and_then(Value::as_u64).unwrap_or(0),
                    result.get("dropped").and_then(Value::as_u64).unwrap_or(0),
                    reply.elapsed_ms
                );
            } else {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
        }
        "sd-list" => {
            agent_sd_list(&args[1..])?;
        }
        "diagnostics" => {
            agent_diagnostics(&args[1..])?;
        }
        "framebuffer-capture" => {
            agent_framebuffer_capture(&args[1..])?;
        }
        "framebuffer-capture-raw" => {
            agent_framebuffer_capture_raw(&args[1..])?;
        }
        "framebuffer-capture-lz4" => {
            agent_framebuffer_capture_lz4(&args[1..])?;
        }
        "magik" => {
            agent_magik(&args[1..])?;
        }
        "reboot-wait" => {
            agent_reboot_wait(&args[1..])?;
        }
        "boot-profile" => {
            agent_boot_profile(&args[1..])?;
        }
        "-h" | "--help" => agent_usage(),
        other => return Err(format!("unknown agent subcommand: {other}").into()),
    }
    Ok(())
}

fn agent_usage() {
    println!(
        "usage: mister agent <ping|status|logs|timeline|sd-list|diagnostics|framebuffer-capture|framebuffer-capture-raw|framebuffer-capture-lz4|magik|reboot-wait|boot-profile>\n       logs [--json]\n       timeline [--json]\n       sd-list PATH [--protocol auto|v1|v2] [--show-hidden] [--repeat N] [--json]\n       diagnostics [--out DIR]\n       framebuffer-capture OUT.png [--json OUT.json]\n       framebuffer-capture-raw OUT.raw [--json OUT.json]\n       framebuffer-capture-lz4 OUT.raw [--json OUT.json]\n       magik <status|suspend|resume|restart-launcher>\n       reboot-wait [--timeout SECS] [--raw|--direct-reset|--direct-reset-no-sync]\n       boot-profile [samples] [--timeout SECS] [--probe-timeout-ms MS] [--sleep-ms MS] [--raw|--direct-reset|--direct-reset-no-sync] [--fail-on-timeout]"
    );
}

fn parse_sd_list_options(args: &[String]) -> Result<SdListOptions> {
    let mut path = None;
    let mut protocol = SdListProtocol::Auto;
    let mut show_hidden = false;
    let mut repeat = 1_usize;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--protocol" => {
                index += 1;
                protocol = SdListProtocol::parse(
                    args.get(index)
                        .ok_or("agent sd-list --protocol needs auto, v1, or v2")?,
                )?;
            }
            "--show-hidden" => show_hidden = true,
            "--repeat" => {
                index += 1;
                repeat = args
                    .get(index)
                    .ok_or("agent sd-list --repeat needs a positive integer")?
                    .parse::<usize>()?;
                if repeat == 0 {
                    return Err("agent sd-list --repeat must be positive".into());
                }
            }
            "--json" => json = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown agent sd-list option: {option}").into());
            }
            value => {
                if path.replace(value.to_string()).is_some() {
                    return Err("agent sd-list takes one PATH".into());
                }
            }
        }
        index += 1;
    }
    Ok(SdListOptions {
        path: path.ok_or("agent sd-list needs PATH")?,
        protocol,
        show_hidden,
        repeat,
        json,
    })
}

fn request_sd_list(
    protocol: SdListProtocol,
    path: &str,
    show_hidden: bool,
) -> Result<(SdListProtocol, agent_client::AgentResponse)> {
    let args = json!({"path": path, "show_hidden": show_hidden});
    match protocol {
        SdListProtocol::V1 => Ok((
            SdListProtocol::V1,
            agent_request("sd_list_dir", args, Duration::from_secs(10))?,
        )),
        SdListProtocol::V2 => Ok((
            SdListProtocol::V2,
            agent_request("sd_list_dir_v2", args, Duration::from_secs(10))?,
        )),
        SdListProtocol::Auto => {
            match agent_request("sd_list_dir_v2", args.clone(), Duration::from_secs(10)) {
                Ok(reply) => Ok((SdListProtocol::V2, reply)),
                Err(err) if err.to_string() == "unknown cmd" => Ok((
                    SdListProtocol::V1,
                    agent_request("sd_list_dir", args, Duration::from_secs(10))?,
                )),
                Err(err) => Err(err),
            }
        }
    }
}

fn agent_sd_list(args: &[String]) -> Result<()> {
    let options = parse_sd_list_options(args)?;
    let mut measurements = Vec::with_capacity(options.repeat);
    for run in 1..=options.repeat {
        let (protocol, reply) =
            request_sd_list(options.protocol, &options.path, options.show_hidden)?;
        let result = reply.response.get("result").unwrap_or(&Value::Null);
        let entries = result
            .get("entries")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let agent_elapsed_ms = result
            .get("elapsed_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let measurement = json!({
            "run": run,
            "requested_protocol": options.protocol.label(),
            "protocol": protocol.label(),
            "path": options.path,
            "show_hidden": options.show_hidden,
            "entries": entries,
            "agent_elapsed_ms": agent_elapsed_ms,
            "round_trip_ms": reply.elapsed_ms,
        });
        if !options.json {
            println!(
                "sd_list\trun={run}\tprotocol={}\tpath={}\tentries={entries}\tagent_ms={agent_elapsed_ms}\tround_trip_ms={}",
                protocol.label(),
                options.path,
                reply.elapsed_ms
            );
        }
        measurements.push(measurement);
    }
    if options.json {
        println!("{}", serde_json::to_string_pretty(&measurements)?);
    }
    Ok(())
}

struct PngCapture {
    result: Value,
    png: Vec<u8>,
    elapsed_ms: u128,
}

fn capture_buffer(args: &[String]) -> Result<()> {
    capture_buffer_at(&AgentEndpoint::from_environment()?, args)
}

fn capture_buffer_at(agent: &AgentEndpoint, args: &[String]) -> Result<()> {
    validate_capture_buffer_args(args)?;
    let capture = request_framebuffer_png_at(agent)?;
    eprintln!(
        "framebuffer capture source={}",
        capture_source_label(&capture.result)?
    );
    if io::stdout().is_terminal() {
        println!("{}", write_desktop_capture(&capture.png)?.display());
    } else {
        let path = write_temporary_capture(&capture.png)?;
        println!("{}", capture_markdown_link(&path));
    }
    Ok(())
}

fn write_temporary_capture(png: &[u8]) -> Result<PathBuf> {
    write_temporary_capture_at(&env::temp_dir(), unix_ms_now(), png)
}

fn write_temporary_capture_at(temp_root: &Path, timestamp_ms: u128, png: &[u8]) -> Result<PathBuf> {
    let directory = temp_root.join("mister-magik").join("captures");
    fs::create_dir_all(&directory)?;
    let directory = fs::canonicalize(directory)?;
    for suffix in 1_u64.. {
        let path = temporary_capture_path(&directory, timestamp_ms, suffix);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(png)?;
                file.sync_all()?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("capture suffix space exhausted")
}

fn temporary_capture_path(directory: &Path, timestamp_ms: u128, suffix: u64) -> PathBuf {
    let suffix = if suffix == 1 {
        String::new()
    } else {
        format!("-{suffix}")
    };
    directory.join(format!(
        "mister-magik-framebuffer-{timestamp_ms}{suffix}.png"
    ))
}

fn capture_markdown_link(path: &Path) -> String {
    format!("[MiSTer framebuffer](<{}>)", path.display())
}

fn delivery_smoke_capture_detail(capture: &PngCapture) -> Result<String> {
    validate_visible_launcher_capture(capture)?;
    let width = capture
        .result
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let height = capture
        .result
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let png_bytes = capture
        .result
        .get("png_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(capture.png.len() as u64);
    Ok(format!(
        "artifact=verified process=healthy module=ready latch=ready screen=recognized input=ready scanout=rgb565 capture=verified width={width} height={height} png_bytes={png_bytes} arming=clear"
    ))
}

fn capture_source_label(result: &Value) -> Result<&str> {
    result
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent framebuffer capture response missing source".into())
}

fn validate_capture_contract(result: &Value) -> Result<()> {
    validate_capture_contract_schema(result, "mister-magik-framebuffer-capture-v2")
}

fn validate_capture_contract_schema(result: &Value, expected_schema: &str) -> Result<()> {
    if result.get("schema").and_then(Value::as_str) != Some(expected_schema) {
        return Err("agent framebuffer capture returned an unsupported schema".into());
    }
    let source = capture_source_label(result)?;
    let source_kind = result
        .get("capture_source")
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        .ok_or("agent framebuffer capture response missing capture_source.kind")?;
    if source != source_kind
        || !matches!(
            source,
            "fb0" | "producer-composition" | "fpga-latched-scanout-slots"
        )
    {
        return Err(format!("agent framebuffer capture returned invalid source {source:?}").into());
    }
    let authoritative_scanout = result
        .get("authoritative_scanout")
        .and_then(Value::as_bool)
        .ok_or("agent framebuffer capture response missing authoritative_scanout")?;
    if authoritative_scanout != (source == "fpga-latched-scanout-slots") {
        return Err("agent framebuffer capture returned inconsistent scanout authority".into());
    }
    result
        .get("content_nonzero_bytes")
        .and_then(Value::as_u64)
        .ok_or("agent framebuffer capture response missing content_nonzero_bytes")?;
    result
        .get("content_varied")
        .and_then(Value::as_bool)
        .ok_or("agent framebuffer capture response missing content_varied")?;
    if source == "fpga-latched-scanout-slots" {
        let metadata = result.get("capture_source").unwrap_or(&Value::Null);
        for field in [
            "active_base",
            "active_sequence",
            "region_index",
            "region_name",
        ] {
            if metadata.get(field).is_none() {
                return Err(format!(
                    "agent framebuffer capture response missing latch field {field}"
                )
                .into());
            }
        }
    } else if source == "producer-composition" {
        let metadata = result.get("capture_source").unwrap_or(&Value::Null);
        for field in ["sequence", "authoritative_error"] {
            if metadata.get(field).is_none() {
                return Err(format!(
                    "agent framebuffer capture response missing producer field {field}"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_visible_launcher_capture(capture: &PngCapture) -> Result<()> {
    let result = &capture.result;
    if capture_source_label(result)? != "fpga-latched-scanout-slots"
        || result.get("authoritative_scanout").and_then(Value::as_bool) != Some(true)
    {
        return Err(format!(
            "launcher capture used non-authoritative source {}",
            capture_source_label(result)?
        )
        .into());
    }
    let width = result.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height = result.get("height").and_then(Value::as_u64).unwrap_or(0);
    let stride = result.get("stride").and_then(Value::as_u64).unwrap_or(0);
    let bpp = result.get("bpp").and_then(Value::as_u64).unwrap_or(0);
    if width == 0 || height == 0 || bpp != 16 || !valid_rgb565_stride(width, stride) {
        return Err(format!(
            "authoritative launcher capture has invalid RGB565 geometry {width}x{height} stride={stride} bpp={bpp}"
        )
        .into());
    }
    let nonzero = result
        .get("content_nonzero_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let varied = result
        .get("content_varied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if nonzero == 0 || !varied {
        return Err("authoritative launcher framebuffer capture is blank or uniform".into());
    }
    Ok(())
}

fn valid_rgb565_stride(width: u64, stride: u64) -> bool {
    width > 0 && stride >= width.saturating_mul(2) && stride.is_multiple_of(2)
}

fn validate_capture_buffer_args(args: &[String]) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("usage: mister --capture-buffer".into())
    }
}

fn request_framebuffer_png() -> Result<PngCapture> {
    request_framebuffer_png_at(&AgentEndpoint::from_environment()?)
}

fn request_framebuffer_png_at(agent: &AgentEndpoint) -> Result<PngCapture> {
    let reply = agent_request_at(
        agent,
        "framebuffer_capture",
        json!({}),
        Duration::from_secs(10),
    )?;
    let result = reply
        .response
        .get("result")
        .ok_or("agent framebuffer capture response missing result")?;
    validate_capture_contract(result)?;
    let png_hex = result
        .get("png_hex")
        .and_then(Value::as_str)
        .ok_or("agent framebuffer capture response missing image data")?;
    let png = decode_hex(png_hex)?;
    validate_png(&png)?;
    Ok(PngCapture {
        result: result.clone(),
        png,
        elapsed_ms: reply.elapsed_ms,
    })
}

fn request_framebuffer_png_at_when_latched(
    agent: &AgentEndpoint,
    timeout: Duration,
) -> Result<PngCapture> {
    let started = Instant::now();
    loop {
        match request_framebuffer_png_at(agent) {
            Ok(capture) => return Ok(capture),
            Err(error)
                if is_transient_authoritative_capture_error(&error.to_string())
                    && started.elapsed() < timeout =>
            {
                thread::sleep(Duration::from_millis(7));
            }
            Err(error) => return Err(error),
        }
    }
}

fn is_transient_authoritative_capture_error(error: &str) -> bool {
    error.contains("latched framebuffer status is not active")
        || error.contains("no scanout slot matches active base")
}

fn validate_png(png: &[u8]) -> Result<()> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("agent framebuffer capture returned invalid PNG data".into());
    }
    Ok(())
}

fn write_desktop_capture(png: &[u8]) -> Result<PathBuf> {
    let desktop = PathBuf::from(env::var("HOME")?).join("Desktop");
    if !desktop.is_dir() {
        return Err(format!("Desktop directory does not exist: {}", desktop.display()).into());
    }
    let output = Command::new("date").arg("+%Y-%m-%d at %H.%M.%S").output()?;
    if !output.status.success() {
        return Err("could not determine local capture time".into());
    }
    let timestamp = String::from_utf8(output.stdout)?.trim().to_string();
    write_desktop_capture_at(&desktop, &timestamp, png)
}

fn write_desktop_capture_at(desktop: &Path, timestamp: &str, png: &[u8]) -> Result<PathBuf> {
    if !desktop.is_dir() {
        return Err(format!("Desktop directory does not exist: {}", desktop.display()).into());
    }
    for suffix in 1_u64.. {
        let path = desktop_capture_path(desktop, timestamp, suffix);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(png)?;
                file.sync_all()?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    unreachable!("capture suffix space exhausted")
}

fn desktop_capture_path(desktop: &Path, timestamp: &str, suffix: u64) -> PathBuf {
    let suffix = if suffix == 1 {
        String::new()
    } else {
        format!(" {suffix}")
    };
    desktop.join(format!("MiSTer Framebuffer {timestamp}{suffix}.png"))
}

fn agent_framebuffer_capture(args: &[String]) -> Result<()> {
    let mut output = None;
    let mut json_output = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--json" => {
                idx += 1;
                let path = args
                    .get(idx)
                    .ok_or("agent framebuffer-capture --json needs OUT.json")?;
                json_output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                println!("usage: mister agent framebuffer-capture OUT.png [--json OUT.json]");
                return Ok(());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown framebuffer-capture option: {other}").into());
            }
            path => {
                if output.is_some() {
                    return Err("agent framebuffer-capture takes one OUT.png path".into());
                }
                output = Some(PathBuf::from(path));
            }
        }
        idx += 1;
    }
    let output = output.ok_or("agent framebuffer-capture needs OUT.png")?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let capture = request_framebuffer_png()?;
    let result = &capture.result;
    fs::write(&output, &capture.png)?;

    let metadata = framebuffer_capture_metadata(result, capture.elapsed_ms, &output);
    if let Some(path) = json_output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(&metadata)?)?;
    }

    let width = result.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height = result.get("height").and_then(Value::as_u64).unwrap_or(0);
    let png_bytes = result
        .get("png_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(capture.png.len() as u64);
    println!(
        "framebuffer_capture: {} ({}x{}, source={}, {}, {}ms)",
        output.display(),
        width,
        height,
        capture_source_label(result)?,
        format_bytes_nearest_kb(png_bytes),
        capture.elapsed_ms
    );
    Ok(())
}

fn agent_framebuffer_capture_raw(args: &[String]) -> Result<()> {
    agent_framebuffer_capture_binary(args, "raw")
}

fn agent_framebuffer_capture_lz4(args: &[String]) -> Result<()> {
    agent_framebuffer_capture_binary(args, "lz4")
}

fn agent_framebuffer_capture_binary(args: &[String], encoding: &str) -> Result<()> {
    let mut output = None;
    let mut json_output = None;
    let command_name = if encoding == "lz4" {
        "framebuffer-capture-lz4"
    } else {
        "framebuffer-capture-raw"
    };
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--json" => {
                idx += 1;
                let path = args
                    .get(idx)
                    .ok_or_else(|| format!("agent {command_name} --json needs OUT.json"))?;
                json_output = Some(PathBuf::from(path));
            }
            "-h" | "--help" => {
                println!("usage: mister agent {command_name} OUT.raw [--json OUT.json]");
                return Ok(());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown {command_name} option: {other}").into());
            }
            path => {
                if output.is_some() {
                    return Err(format!("agent {command_name} takes one OUT.raw path").into());
                }
                output = Some(PathBuf::from(path));
            }
        }
        idx += 1;
    }
    let output = output.ok_or_else(|| format!("agent {command_name} needs OUT.raw"))?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let agent_command = if encoding == "lz4" {
        "framebuffer_capture_lz4_stream"
    } else {
        "framebuffer_capture_raw_stream"
    };
    let reply = agent_binary_request_bounded(
        agent_command,
        json!({}),
        Duration::from_secs(10),
        MAX_FRAMEBUFFER_CAPTURE_PAYLOAD_BYTES,
    )?;
    let result = reply
        .response
        .get("result")
        .ok_or("agent framebuffer binary response missing result")?;
    validate_capture_contract_schema(result, "mister-magik-framebuffer-raw-stream-v2")?;
    let expected_raw = result
        .get("raw_bytes")
        .and_then(Value::as_u64)
        .map(usize::try_from)
        .transpose()?
        .ok_or("agent framebuffer response missing raw_bytes")?;
    if expected_raw > MAX_FRAMEBUFFER_CAPTURE_RAW_BYTES {
        return Err(
            format!("framebuffer capture raw payload too large: {expected_raw} bytes").into(),
        );
    }
    let raw = if encoding == "lz4" {
        decompress_framebuffer_capture_lz4(&reply.payload, expected_raw)?
    } else {
        reply.payload.clone()
    };
    if raw.len() != expected_raw {
        return Err(format!(
            "decoded framebuffer size mismatch expected={expected_raw} actual={}",
            raw.len()
        )
        .into());
    }
    fs::write(&output, &raw)?;
    let metadata = framebuffer_capture_raw_metadata(result, reply.elapsed_ms, &output);
    if let Some(path) = json_output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(&metadata)?)?;
    }

    let width = result.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height = result.get("height").and_then(Value::as_u64).unwrap_or(0);
    let bpp = result.get("bpp").and_then(Value::as_u64).unwrap_or(0);
    println!(
        "framebuffer_capture_{encoding}: {} ({}x{} {}bpp, {} payload, {} raw, {}ms)",
        output.display(),
        width,
        height,
        bpp,
        format_bytes_nearest_kb(reply.payload.len() as u64),
        format_bytes_nearest_kb(raw.len() as u64),
        reply.elapsed_ms
    );
    Ok(())
}

fn decompress_framebuffer_capture_lz4(payload: &[u8], expected_raw: usize) -> Result<Vec<u8>> {
    let (prefixed_raw, compressed) = lz4_flex::block::uncompressed_size(payload)?;
    if prefixed_raw != expected_raw {
        return Err(format!(
            "framebuffer capture LZ4 size prefix mismatch expected={expected_raw} actual={prefixed_raw}"
        )
        .into());
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(expected_raw)?;
    raw.resize(expected_raw, 0);
    let actual = lz4_flex::block::decompress_into(compressed, &mut raw)?;
    if actual != expected_raw {
        return Err(format!(
            "decoded framebuffer size mismatch expected={expected_raw} actual={actual}"
        )
        .into());
    }
    Ok(raw)
}

fn framebuffer_capture_metadata(result: &Value, request_ms: u128, output: &Path) -> Value {
    let mut metadata = result.clone();
    if let Value::Object(ref mut object) = metadata {
        object.remove("png_hex");
        object.insert("transport".to_string(), Value::String("agent".to_string()));
        object.insert("request_ms".to_string(), Value::from(request_ms as u64));
        object.insert(
            "png_path".to_string(),
            Value::String(output.display().to_string()),
        );
    }
    metadata
}

fn framebuffer_capture_raw_metadata(result: &Value, request_ms: u128, output: &Path) -> Value {
    let mut metadata = result.clone();
    if let Value::Object(ref mut object) = metadata {
        object.insert(
            "transport".to_string(),
            Value::String("agent-raw-stream".to_string()),
        );
        object.insert("request_ms".to_string(), Value::from(request_ms as u64));
        object.insert(
            "raw_path".to_string(),
            Value::String(output.display().to_string()),
        );
    }
    metadata
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return Err("hex payload has odd length".into());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let raw = hex.as_bytes();
    let mut idx = 0;
    while idx < raw.len() {
        let hi = hex_value(raw[idx])?;
        let lo = hex_value(raw[idx + 1])?;
        bytes.push((hi << 4) | lo);
        idx += 2;
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex byte: {byte}").into()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn format_bytes_nearest_kb(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    } else {
        format!("{} KB", (bytes + 512) / 1024)
    }
}

fn agent_magik(args: &[String]) -> Result<()> {
    let json_output = args.iter().any(|arg| arg == "--json");
    let positional = args
        .iter()
        .filter(|arg| arg.as_str() != "--json")
        .collect::<Vec<_>>();
    let action = positional
        .first()
        .map(|value| value.as_str())
        .unwrap_or("status");
    match action {
        "status" | "suspend" | "resume" | "restart-launcher" | "return-to-launcher"
        | "exit-to-menu" | "launch" => {}
        "-h" | "--help" => {
            println!(
                "usage: mister agent magik <status|suspend|resume|restart-launcher|return-to-launcher|exit-to-menu|launch TARGET> [--json]"
            );
            return Ok(());
        }
        other => return Err(format!("unknown agent magik action: {other}").into()),
    }
    let operation_id = format!(
        "host-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
    );
    let expected_generation = if action == "status" {
        None
    } else {
        let status = agent_request("magik", json!({"action": "status"}), Duration::from_secs(5))?;
        Some(
            status
                .response
                .pointer("/result/files/main_status/main_generation")
                .and_then(Value::as_u64)
                .ok_or("agent Main status missing generation")?,
        )
    };
    let target = positional.get(1).map(|value| value.as_str());
    if (action == "launch") != target.is_some() || positional.len() > 2 {
        return Err("usage: mister agent magik <status|suspend|resume|restart-launcher|return-to-launcher|exit-to-menu|launch TARGET> [--json]".into());
    }
    let request = json!({"action": action, "operation_id": operation_id, "expected_generation": expected_generation, "target": target});
    let reply = if action == "status" {
        agent_request("magik", request, Duration::from_secs(5))?
    } else {
        agent_request_with_liveness("magik", request, Duration::from_secs(5))?
    };
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    if json_output {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!(
            "{}",
            format_agent_magik_summary(action, reply.elapsed_ms, result)
        );
    }
    Ok(())
}

fn format_agent_magik_summary(action: &str, request_ms: u128, result: &Value) -> String {
    let status = if action == "status" {
        result.pointer("/files/main_status").unwrap_or(&Value::Null)
    } else {
        result.get("main_status").unwrap_or(&Value::Null)
    };
    let state = status
        .get("launcher_state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let pid = status
        .get("launcher_pid")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let generation = status
        .get("main_generation")
        .and_then(Value::as_u64)
        .or_else(|| result.get("after_generation").and_then(Value::as_u64))
        .unwrap_or(0);
    let outcome = result
        .get("terminal_reason")
        .and_then(Value::as_str)
        .unwrap_or(if action == "status" {
            "ok"
        } else {
            "acknowledged"
        });
    format!(
        "agent magik action={action} outcome={outcome} elapsed_ms={request_ms} state={state} pid={pid} generation={generation}"
    )
}

fn agent_reboot_wait(args: &[String]) -> Result<()> {
    let reboot_mode = reboot_mode_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            args.iter()
                .find(|arg| !arg.starts_with('-'))
                .and_then(|arg| arg.parse::<f64>().ok())
        })
        .unwrap_or(40.0);
    let mode = reboot_mode.label();
    let issue_t = Instant::now();
    if reboot_mode.is_direct_reset() {
        let sess = connect(10)?;
        let issued = issue_reboot(&sess, reboot_mode)?;
        let issue_ms = issue_t.elapsed().as_millis();
        println!(
            "agent reboot issued to {} after {issue_ms}ms: {issued}",
            host()
        );
    } else {
        let reply = agent_request("reboot", json!({"mode": mode}), Duration::from_secs(2))?;
        let issue_ms = issue_t.elapsed().as_millis();
        println!(
            "agent reboot issued to {} after {issue_ms}ms: {}",
            host(),
            serde_json::to_string(reply.response.get("result").unwrap_or(&Value::Null))?
        );
    }

    let start = Instant::now();
    let mut down_ms = None;
    while start.elapsed().as_secs_f64() < 40.0 {
        let ssh_label = tcp_probe_label(Duration::from_millis(100));
        let agent_label = tcp_probe_label_port(AGENT_PORT, Duration::from_millis(100));
        if ssh_label != "ok" && agent_label != "ok" {
            down_ms = Some(start.elapsed().as_millis());
            println!("  device went down after {}ms", opt_ms(down_ms));
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    let mut agent_ready_ms = None;
    let mut ssh_ready_ms = None;
    let mut last_note = String::new();
    while start.elapsed().as_secs_f64() < timeout_secs {
        if agent_ready_ms.is_none() {
            let agent_probe = agent_request("ping", json!({}), Duration::from_millis(300));
            match agent_probe {
                Ok(_) => {
                    agent_ready_ms = Some(start.elapsed().as_millis());
                    println!("  agent ready after {}ms", opt_ms(agent_ready_ms));
                }
                Err(err) => last_note = err.to_string(),
            }
        }
        if ssh_ready_ms.is_none() {
            let ssh_probe = connect_timed(2);
            match ssh_probe {
                Ok(timed) => {
                    let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                    if out.rc == 0 {
                        ssh_ready_ms = Some(start.elapsed().as_millis());
                        let ssh_uptime = out.stdout.split_whitespace().next().unwrap_or("");
                        println!(
                            "  ssh ready after {}ms; uptime={ssh_uptime}",
                            opt_ms(ssh_ready_ms)
                        );
                    } else {
                        last_note = format!("ssh exec rc {}", out.rc);
                    }
                }
                Err(err) => last_note = err.to_string(),
            }
        }
        if agent_ready_ms.is_some() && ssh_ready_ms.is_some() {
            if down_ms.is_none() {
                return Err(format!(
                    "agent reboot-wait did not observe the device go down; refusing to treat the existing connection as a {mode} reboot"
                )
                .into());
            }
            println!(
                "agent reboot-wait ok mode={mode} down_ms={} agent_ready_ms={} ssh_ready_ms={}",
                opt_ms(down_ms),
                opt_ms(agent_ready_ms),
                opt_ms(ssh_ready_ms)
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(150));
    }

    Err(format!(
        "agent reboot-wait timeout mode={mode} down_ms={} agent_ready_ms={} ssh_ready_ms={} last={}",
        opt_ms(down_ms),
        opt_ms(agent_ready_ms),
        opt_ms(ssh_ready_ms),
        last_note
    )
    .into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LauncherRestartOptions {
    env_vars: Vec<(String, String)>,
    clear_env: bool,
    timeout_secs: u64,
    remote_env: String,
}

impl Default for LauncherRestartOptions {
    fn default() -> Self {
        Self {
            env_vars: Vec::new(),
            clear_env: false,
            timeout_secs: 20,
            remote_env: configured_remote_path(
                "MISTER_MAGIK_LAUNCHER_ENV",
                DEFAULT_LAUNCHER_ENV_REMOTE,
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LauncherReadyStatus {
    main_ms: u128,
    slint_ms: u128,
    launcher_pid: i64,
    slint_pid: i64,
    frames: u64,
    screen: String,
}

fn launcher_restart(sess: &Session, options: &LauncherRestartOptions) -> Result<()> {
    let started = Instant::now();
    let env_t = Instant::now();
    let env_mode = prepare_launcher_env(sess, options)?;
    let env_ms = env_t.elapsed().as_millis();

    let issue_t = Instant::now();
    issue_launcher_restart(sess)?;
    let issue_ms = issue_t.elapsed().as_millis();

    let ready = wait_launcher_ready(sess, started, Duration::from_secs(options.timeout_secs))?;
    println!(
        "launcher restart ok host={} env={} env_ms={} issue_ms={} ready_ms={} main_status_ms={} slint_status_ms={} launcher_pid={} slint_pid={} frames={} screen={}",
        host(),
        env_mode,
        env_ms,
        issue_ms,
        started.elapsed().as_millis(),
        ready.main_ms,
        ready.slint_ms,
        ready.launcher_pid,
        ready.slint_pid,
        ready.frames,
        ready.screen
    );
    Ok(())
}

fn launcher_restart_help_requested(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
}

fn launcher_restart_usage() {
    println!(
        "usage: mister launcher-restart [--env KEY=VALUE]... [--clear-env] [--timeout SECS] [--remote-env PATH]"
    );
}

fn parse_launcher_restart_args(args: &[String]) -> Result<LauncherRestartOptions> {
    let mut options = LauncherRestartOptions::default();
    let mut idx = 0usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--env" => {
                idx += 1;
                let item = args
                    .get(idx)
                    .ok_or("launcher-restart --env needs KEY=VALUE")?;
                let (key, value) = parse_launcher_env_pair(item)?;
                options.env_vars.push((key, value));
            }
            "--clear-env" => {
                options.clear_env = true;
            }
            "--timeout" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .ok_or("launcher-restart --timeout needs seconds")?;
                options.timeout_secs = value.parse::<u64>().map_err(|_| {
                    format!("launcher-restart --timeout must be an integer: {value}")
                })?;
                if options.timeout_secs == 0 {
                    return Err("launcher-restart --timeout must be positive".into());
                }
            }
            "--remote-env" => {
                idx += 1;
                options.remote_env = args
                    .get(idx)
                    .ok_or("launcher-restart --remote-env needs a path")?
                    .clone();
            }
            "-h" | "--help" => launcher_restart_usage(),
            other => return Err(format!("unknown launcher-restart option: {other}").into()),
        }
        idx += 1;
    }
    if options.clear_env && !options.env_vars.is_empty() {
        return Err("launcher-restart cannot combine --clear-env with --env".into());
    }
    let _ = remote_parent_dir(&options.remote_env)?;
    Ok(options)
}

fn parse_launcher_env_pair(item: &str) -> Result<(String, String)> {
    let (key, value) = item
        .split_once('=')
        .ok_or_else(|| format!("launcher env must be KEY=VALUE: {item}"))?;
    if !is_launcher_env_key(key) {
        return Err(format!("invalid launcher env key: {key}").into());
    }
    Ok((key.to_string(), value.to_string()))
}

fn is_launcher_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn launcher_env_text(vars: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in vars {
        text.push_str("export ");
        text.push_str(key);
        text.push('=');
        text.push_str(&shell_export_quote(value));
        text.push('\n');
    }
    text
}

fn shell_export_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn prepare_launcher_env(sess: &Session, options: &LauncherRestartOptions) -> Result<String> {
    if options.clear_env {
        let out = exec(sess, &remove_files_command(&[&options.remote_env]), true)?;
        if let Some(error) = exec_failure_message("clear launcher env", &out) {
            return Err(error.into());
        }
        return Ok("cleared".to_string());
    }
    if options.env_vars.is_empty() {
        return Ok("unchanged".to_string());
    }
    let parent = remote_parent_dir(&options.remote_env)?;
    let out = exec(sess, &create_dir_command(parent), true)?;
    if let Some(error) = exec_failure_message("create launcher env parent", &out) {
        return Err(error.into());
    }
    put_bytes(
        sess,
        &options.remote_env,
        launcher_env_text(&options.env_vars).as_bytes(),
    )?;
    Ok(format!("written:{}", options.env_vars.len()))
}

fn remote_parent_dir(remote: &str) -> Result<&str> {
    if !remote.starts_with('/') {
        return Err(
            format!("remote path must be absolute and include a directory: {remote}").into(),
        );
    }
    remote
        .rsplit_once('/')
        .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
        .ok_or_else(|| {
            format!("remote path must be absolute and include a directory: {remote}").into()
        })
}

fn issue_launcher_restart(sess: &Session) -> Result<()> {
    let command = launcher_restart_command(MAIN_STATUS_REMOTE, SLINT_STATUS_REMOTE);
    let out = exec(sess, &command, true)?;
    if let Some(error) = exec_failure_message("launcher restart command", &out) {
        return Err(error.into());
    }
    Ok(())
}

fn wait_launcher_ready(
    sess: &Session,
    started: Instant,
    timeout: Duration,
) -> Result<LauncherReadyStatus> {
    let mut last_state = String::new();
    while started.elapsed() < timeout {
        let elapsed_ms = started.elapsed().as_millis();
        let main = remote_read(sess, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let slint = remote_read(sess, SLINT_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let state = main
            .as_ref()
            .and_then(|value| value.get("launcher_state"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        last_state = state.to_string();
        if let Some(ready) = launcher_ready_status(elapsed_ms, main.as_ref(), slint.as_ref()) {
            return Ok(ready);
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "launcher restart timed out after {}ms; last launcher_state={last_state}",
        timeout.as_millis()
    )
    .into())
}

fn wait_launcher_ready_after(
    sess: &Session,
    previous_pid: i64,
    started: Instant,
    timeout: Duration,
) -> Result<LauncherReadyStatus> {
    let mut last_pid = previous_pid;
    while started.elapsed() < timeout {
        let elapsed_ms = started.elapsed().as_millis();
        let main = remote_read(sess, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let slint = remote_read(sess, SLINT_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        if let Some(ready) = launcher_ready_status(elapsed_ms, main.as_ref(), slint.as_ref()) {
            last_pid = ready.launcher_pid;
            if ready.launcher_pid != previous_pid {
                return Ok(ready);
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(
        format!("launcher did not restart after pid {previous_pid}; last launcher pid={last_pid}")
            .into(),
    )
}

fn launcher_ready_status(
    elapsed_ms: u128,
    main: Option<&Value>,
    slint: Option<&Value>,
) -> Option<LauncherReadyStatus> {
    let main = main?;
    let slint = slint?;
    if main.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive") {
        return None;
    }
    if slint.get("scene").and_then(Value::as_str) != Some("launcher") {
        return None;
    }
    let launcher_pid = main.get("launcher_pid").and_then(Value::as_i64)?;
    let slint_pid = slint.get("pid").and_then(Value::as_i64)?;
    if launcher_pid <= 0 || launcher_pid != slint_pid {
        return None;
    }
    let frames = slint.get("frames").and_then(Value::as_u64).unwrap_or(0);
    if frames == 0 {
        return None;
    }
    Some(LauncherReadyStatus {
        main_ms: elapsed_ms,
        slint_ms: elapsed_ms,
        launcher_pid,
        slint_pid,
        frames,
        screen: slint
            .get("screen")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn agent_diagnostics(args: &[String]) -> Result<()> {
    let out_dir = option_value(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("build/agent-diagnostics/{}", unix_secs())));
    fs::create_dir_all(&out_dir)?;

    let bundle = match agent_request("diagnostics", json!({}), Duration::from_secs(3)) {
        Ok(reply) => {
            let mut result = reply.response.get("result").cloned().unwrap_or(Value::Null);
            if let Value::Object(ref mut object) = result {
                object.insert("transport".to_string(), Value::String("agent".to_string()));
                object.insert(
                    "request_ms".to_string(),
                    Value::from(reply.elapsed_ms as u64),
                );
            }
            result
        }
        Err(err) => {
            eprintln!("agent diagnostics unavailable over TCP: {err}; falling back to SSH");
            ssh_diagnostics_bundle(err.to_string())?
        }
    };

    write_diagnostics_bundle(&out_dir, &bundle)?;
    println!("diagnostics_dir={}", out_dir.display());
    Ok(())
}

fn ssh_diagnostics_bundle(agent_error: String) -> Result<Value> {
    let sess = connect(10)?;
    let status = collect_status(&sess)?;
    let ps = exec(&sess, "ps w", true)
        .map(|out| out.stdout)
        .unwrap_or_else(|err| format!("error: {err}"));
    Ok(json!({
        "schema": "mister-magik-agent-diagnostics-v1",
        "transport": "ssh-fallback",
        "agent_error": agent_error,
        "status": status,
        "timeline": Value::Null,
        "agent_logs": Value::Null,
        "net": {
            "carrier": remote_read(&sess, "/sys/class/net/eth0/carrier"),
            "operstate": remote_read(&sess, "/sys/class/net/eth0/operstate"),
            "address": remote_read(&sess, "/sys/class/net/eth0/address"),
            "route": remote_read(&sess, "/proc/net/route"),
            "arp": remote_read(&sess, "/proc/net/arp"),
            "dev": remote_read(&sess, "/proc/net/dev"),
        },
        "processes": {
            "ps": ps,
        },
        "files": {
            "slint_status": remote_read(&sess, "/tmp/mister-magik/status.json"),
            "main_status": remote_read(&sess, "/tmp/mister-magik/main-status.json"),
            "events_tail": tail_remote(&sess, "/tmp/mister-magik/events.jsonl", 80).map(|lines| lines.join("\n")),
            "slint_log_tail": tail_remote(&sess, "/tmp/mister-magik-slint.log", 120).map(|lines| lines.join("\n")),
            "main_log_tail": tail_remote(&sess, "/tmp/mister-magik-main.log", 120).map(|lines| lines.join("\n")),
            "agent_tmp_log_tail": tail_remote(&sess, "/tmp/mister-magik-agent.log", 160).map(|lines| lines.join("\n")),
            "agent_persistent_log_tail": tail_remote(&sess, "/media/fat/mister-magik-dev/bootlogs/agent.log", 160).map(|lines| lines.join("\n")),
            "boot_analytics_tail": tail_remote(&sess, "/tmp/mister-magik-boot-analytics.tsv", 80).map(|lines| lines.join("\n")),
        },
        "crashes": ssh_crash_reports_json(&sess),
        "catalog_failures": ssh_catalog_failure_reports_json(&sess),
        "catalog_progress": ssh_latest_diagnostic_report(
            &sess,
            "diagnostics/catalog/progress-latest.json",
            "updated_unix_ms",
        ),
        "latch_failure": ssh_current_latch_failure_report(&sess),
    }))
}

fn write_diagnostics_bundle(out_dir: &Path, bundle: &Value) -> Result<()> {
    fs::write(
        out_dir.join("bundle.json"),
        serde_json::to_vec_pretty(bundle)?,
    )?;
    write_json_member(out_dir, "status.json", bundle.get("status"))?;
    write_json_member(out_dir, "timeline.json", bundle.get("timeline"))?;
    write_json_member(out_dir, "agent-logs.json", bundle.get("agent_logs"))?;
    write_json_member(out_dir, "net.json", bundle.get("net"))?;
    write_json_member(out_dir, "processes.json", bundle.get("processes"))?;
    write_json_member(out_dir, "crashes.json", bundle.get("crashes"))?;
    write_json_member(
        out_dir,
        "crash-latest.json",
        bundle.pointer("/crashes/latest"),
    )?;
    write_json_member(
        out_dir,
        "catalog-failures.json",
        bundle.get("catalog_failures"),
    )?;
    write_json_member(
        out_dir,
        "catalog-failure-latest.json",
        bundle.pointer("/catalog_failures/latest/report"),
    )?;
    write_json_member(
        out_dir,
        "catalog-progress-latest.json",
        bundle.pointer("/catalog_progress/report"),
    )?;
    write_json_member(
        out_dir,
        "latch-failure-latest.json",
        bundle.pointer("/latch_failure/report"),
    )?;

    write_string_pointer(out_dir, "ps.txt", bundle.pointer("/processes/ps"))?;
    write_string_pointer(
        out_dir,
        "slint-status.json",
        bundle.pointer("/files/slint_status"),
    )?;
    write_string_pointer(
        out_dir,
        "main-status.json",
        bundle.pointer("/files/main_status"),
    )?;
    write_string_pointer(
        out_dir,
        "events-tail.jsonl",
        bundle.pointer("/files/events_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "slint-log-tail.log",
        bundle.pointer("/files/slint_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "main-log-tail.log",
        bundle.pointer("/files/main_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "agent-tmp-log-tail.log",
        bundle.pointer("/files/agent_tmp_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "agent-persistent-log-tail.log",
        bundle.pointer("/files/agent_persistent_log_tail"),
    )?;
    write_string_pointer(
        out_dir,
        "boot-analytics-tail.tsv",
        bundle.pointer("/files/boot_analytics_tail"),
    )?;
    Ok(())
}

fn ssh_crash_reports_json(sess: &Session) -> Value {
    let crash_dir =
        configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik") + "/crashes";
    let latest_path = format!("{crash_dir}/latest.json");
    let latest = remote_read(sess, &latest_path)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let latest_report_id = latest
        .get("report_id")
        .and_then(Value::as_str)
        .map(|report_id| format!("{report_id}.json"));
    let recent = remote_crash_report_paths(sess, 5, latest_report_id.as_deref())
        .into_iter()
        .map(|path| {
            let report = remote_read(sess, &path)
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            json!({
                "path": path,
                "report": report,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "dir": crash_dir,
        "latest_path": latest_path,
        "latest": latest,
        "recent": recent,
    })
}

fn ssh_catalog_failure_reports_json(sess: &Session) -> Value {
    let configured = configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik")
        + "/diagnostics/catalog";
    let mut dirs = vec![
        configured,
        "/media/fat/mister-magik/diagnostics/catalog".to_string(),
        "/media/fat/mister-magik-dev/diagnostics/catalog".to_string(),
    ];
    dirs.sort();
    dirs.dedup();
    let mut latest = dirs
        .iter()
        .filter_map(|dir| {
            let path = format!("{dir}/latest.json");
            let report = remote_read(sess, &path)
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
            Some((path, report))
        })
        .collect::<Vec<_>>();
    latest.sort_by_key(|(_, report)| {
        report
            .get("ts_unix_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    });
    let latest = latest
        .pop()
        .map(|(path, report)| json!({"path": path, "report": report}))
        .unwrap_or(Value::Null);
    let mut recent = dirs
        .iter()
        .flat_map(|dir| remote_catalog_failure_paths(sess, dir, 5))
        .collect::<Vec<_>>();
    recent.sort_by(|left, right| left.rsplit('/').next().cmp(&right.rsplit('/').next()));
    recent.dedup();
    recent.reverse();
    recent.truncate(5);
    json!({
        "latest": latest,
        "recent_paths": recent,
    })
}

fn ssh_latest_diagnostic_report(
    sess: &Session,
    relative_path: &str,
    timestamp_field: &str,
) -> Value {
    let configured = configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik");
    let mut paths = vec![
        format!("{configured}/{relative_path}"),
        format!("/media/fat/mister-magik/{relative_path}"),
        format!("/media/fat/mister-magik-dev/{relative_path}"),
    ];
    paths.sort();
    paths.dedup();
    let mut reports = paths
        .into_iter()
        .filter_map(|path| {
            let report = remote_read(sess, &path)
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
            Some((path, report))
        })
        .collect::<Vec<_>>();
    reports.sort_by_key(|(_, report)| {
        report
            .get(timestamp_field)
            .and_then(Value::as_u64)
            .unwrap_or(0)
    });
    reports
        .pop()
        .map(|(path, report)| json!({"path": path, "report": report}))
        .unwrap_or(Value::Null)
}

fn ssh_current_latch_failure_report(sess: &Session) -> Value {
    for app in [
        configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik"),
        "/media/fat/mister-magik".to_owned(),
        "/media/fat/mister-magik-dev".to_owned(),
    ] {
        let pointer_path = format!("{app}/diagnostics/latch/current-identity.json");
        let Some(pointer) = remote_read(sess, &pointer_path)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let Some(relative) = pointer.get("latest_relative_path").and_then(Value::as_str) else {
            continue;
        };
        if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
            continue;
        }
        let report_path = format!("{app}/diagnostics/latch/{relative}");
        let Some(report) = remote_read(sess, &report_path)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        if report.get("schema").and_then(Value::as_str)
            == Some("mister-magik-latch-failure-report-v2")
            && report.get("identity") == pointer.get("identity")
        {
            return json!({
                "path": report_path,
                "identity_pointer": pointer_path,
                "report": report,
            });
        }
    }
    Value::Null
}

fn remote_catalog_failure_paths(sess: &Session, dir: &str, limit: usize) -> Vec<String> {
    let cmd = format!(
        "ls -1 {} 2>/dev/null | grep '^report-catalog-.*\\.json$' | sort -r | head -n {}",
        sh(dir),
        limit
    );
    let Ok(out) = exec(sess, &cmd, true) else {
        return Vec::new();
    };
    if out.rc != 0 {
        return Vec::new();
    }
    out.stdout
        .lines()
        .map(|name| format!("{dir}/{name}"))
        .collect()
}

fn remote_crash_report_paths(
    sess: &Session,
    limit: usize,
    latest_name: Option<&str>,
) -> Vec<String> {
    let crash_dir =
        configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik") + "/crashes";
    let cmd = format!(
        "ls -1 {} 2>/dev/null | grep '^report-.*\\.json$' | sort | tail -n {}",
        sh(&crash_dir),
        limit
    );
    let Ok(out) = exec(sess, &cmd, true) else {
        return Vec::new();
    };
    if out.rc != 0 {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(name) = latest_name {
        paths.push(format!("{crash_dir}/{name}"));
    }
    paths.extend(
        out.stdout
            .lines()
            .filter(|line| Some(*line) != latest_name)
            .map(|line| format!("{crash_dir}/{line}")),
    );
    paths.truncate(limit);
    paths
}

fn write_json_member(out_dir: &Path, name: &str, value: Option<&Value>) -> Result<()> {
    if let Some(value) = value
        && !value.is_null()
    {
        fs::write(out_dir.join(name), serde_json::to_vec_pretty(value)?)?;
    }
    Ok(())
}

fn write_string_pointer(out_dir: &Path, name: &str, value: Option<&Value>) -> Result<()> {
    if let Some(text) = value.and_then(Value::as_str) {
        fs::write(out_dir.join(name), text)?;
    }
    Ok(())
}

fn agent_probe_label(timeout: Duration) -> String {
    match agent_request("ping", json!({}), timeout) {
        Ok(_) => "ok".to_string(),
        Err(err) => {
            let text = err.to_string();
            if text.contains("Connection refused") || text.contains("connection refused") {
                "refused".to_string()
            } else if text.contains("timed out") || text.contains("TimedOut") {
                "timeout".to_string()
            } else if text.contains("No route to host") {
                "noroute".to_string()
            } else if text.contains("Host is down") {
                "hostdown".to_string()
            } else {
                text.replace('\t', " ").replace(' ', "_")
            }
        }
    }
}

fn agent_boot_profile(args: &[String]) -> Result<()> {
    let _ = agent_token()?;
    let samples = parse_profile_count(args, 1);
    let reboot_mode = reboot_mode_from_args(args)?;
    let fail_on_timeout = args.iter().any(|arg| arg == "--fail-on-timeout");
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(40.0);
    let probe_timeout_ms = option_value(args, "--probe-timeout-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(100);
    let sleep_ms = option_value(args, "--sleep-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50);
    let mode = reboot_mode.label();
    let out_path = "history/toolchain-bench/results-agent.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\tagent_ready_ms\tssh_exec_ready_ms\tagent_first_hostdown_ms\tagent_first_noroute_ms\tagent_first_timeout_ms\tagent_first_refused_ms\tagent_first_other_ms\tagent_ok_count\tagent_hostdown_count\tagent_noroute_count\tagent_timeout_count\tagent_refused_count\tagent_other_count\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tagent_uptime_ms\tssh_uptime\tagent_transitions\tnote";
    println!("{header}");

    let mut recovered = 0usize;
    let mut worst_agent_ready_ms: Option<u128> = None;
    let mut worst_ssh_ready_ms: Option<u128> = None;
    let mut total_noroute = 0u64;
    let mut total_timeout = 0u64;
    let mut total_refused = 0u64;

    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, reboot_mode)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            let ssh_label = tcp_probe_label(Duration::from_millis(100));
            let agent_label = tcp_probe_label_port(AGENT_PORT, Duration::from_millis(100));
            if ssh_label != "ok" && agent_label != "ok" {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let mut agent_stats = TcpProbeStats::default();
        let mut agent_ready_ms = None;
        let mut agent_uptime_ms = String::new();
        let mut ssh_ready_ms = None;
        let mut resolve_ms = None;
        let mut tcp_ms = None;
        let mut handshake_ms = None;
        let mut auth_ms = None;
        let mut exec_ms = None;
        let mut ssh_uptime = String::new();
        let mut main_status_ms = None;
        let mut launcher_state = String::new();
        let mut note = reboot_note;
        let mut first_agent_net = None;
        let mut final_agent_net = None;

        while start.elapsed().as_secs_f64() < timeout_secs {
            let elapsed_ms = start.elapsed().as_millis();
            if agent_ready_ms.is_none() {
                let label = agent_probe_label(Duration::from_millis(probe_timeout_ms));
                agent_stats.observe(&label, elapsed_ms);
                if label == "ok" {
                    agent_ready_ms = Some(elapsed_ms);
                    let status = agent_request("status", json!({}), Duration::from_millis(500));
                    if let Ok(reply) = status {
                        first_agent_net = agent_net_snapshot(&reply.response);
                        agent_uptime_ms = reply
                            .response
                            .pointer("/result/agent/uptime_ms")
                            .and_then(Value::as_u64)
                            .map(|n| n.to_string())
                            .unwrap_or_default();
                    }
                }
            }

            if ssh_ready_ms.is_none() {
                let ssh_probe = connect_timed(2);
                match ssh_probe {
                    Ok(timed) => {
                        let exec_t = Instant::now();
                        let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                        let this_exec_ms = exec_t.elapsed().as_millis();
                        if out.rc == 0 {
                            ssh_ready_ms = Some(start.elapsed().as_millis());
                            resolve_ms = Some(timed.resolve_ms);
                            tcp_ms = Some(timed.tcp_ms);
                            handshake_ms = Some(timed.handshake_ms);
                            auth_ms = Some(timed.auth_ms);
                            exec_ms = Some(this_exec_ms);
                            ssh_uptime = out
                                .stdout
                                .split_whitespace()
                                .next()
                                .unwrap_or("")
                                .to_string();

                            let status_deadline = Instant::now() + Duration::from_secs(20);
                            while Instant::now() < status_deadline
                                && start.elapsed().as_secs_f64() < timeout_secs
                            {
                                if let Some(text) =
                                    remote_read(&timed.sess, "/tmp/mister-magik/main-status.json")
                                    && let Ok(value) = serde_json::from_str::<Value>(&text)
                                {
                                    main_status_ms = Some(start.elapsed().as_millis());
                                    launcher_state = value
                                        .get("launcher_state")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string();
                                    if launcher_state == "LauncherActive" {
                                        break;
                                    }
                                }
                                thread::sleep(Duration::from_millis(250));
                            }
                        } else {
                            note = format!("exec rc {}", out.rc);
                        }
                    }
                    Err(err) => {
                        note = err.to_string();
                    }
                }
            }

            if agent_ready_ms.is_some()
                && ssh_ready_ms.is_some()
                && launcher_state == "LauncherActive"
            {
                break;
            }
            thread::sleep(Duration::from_millis(sleep_ms));
        }

        if agent_ready_ms.is_some() {
            let status = agent_request("status", json!({}), Duration::from_millis(500));
            if let Ok(reply) = status {
                final_agent_net = agent_net_snapshot(&reply.response);
            }
        }

        let agent_rx_delta = first_agent_net
            .as_ref()
            .zip(final_agent_net.as_ref())
            .map(|(first, final_)| final_.rx_packets.saturating_sub(first.rx_packets));
        let agent_tx_delta = first_agent_net
            .as_ref()
            .zip(final_agent_net.as_ref())
            .map(|(first, final_)| final_.tx_packets.saturating_sub(first.tx_packets));
        let agent_carrier = final_agent_net
            .as_ref()
            .map(|snapshot| snapshot.carrier.as_str())
            .unwrap_or("missing");
        let agent_final_rx_packets = final_agent_net.as_ref().map(|snapshot| snapshot.rx_packets);
        let agent_final_tx_packets = final_agent_net.as_ref().map(|snapshot| snapshot.tx_packets);
        let agent_rx_increasing = agent_rx_delta.map(|delta| delta > 0).unwrap_or(false);
        let agent_rx_nonzero = agent_final_rx_packets
            .map(|packets| packets > 0)
            .unwrap_or(false);
        let transitions = agent_stats.transitions.join(",");
        let note = format!(
            "{} main_status_ms={} launcher_state={} agent_carrier={} agent_rx_packets={} agent_tx_packets={} agent_rx_delta={} agent_tx_delta={} agent_rx_increasing={} agent_rx_nonzero={}",
            note,
            opt_ms(main_status_ms),
            if launcher_state.is_empty() {
                "missing"
            } else {
                &launcher_state
            },
            agent_carrier,
            agent_final_rx_packets
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            agent_final_tx_packets
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            agent_rx_delta
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            agent_tx_delta
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            u8::from(agent_rx_increasing),
            u8::from(agent_rx_nonzero)
        );
        let row = format!(
            "agent-boot\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{agent_uptime_ms}\t{ssh_uptime}\t{}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(agent_ready_ms),
            opt_ms(ssh_ready_ms),
            opt_ms(agent_stats.first_hostdown_ms),
            opt_ms(agent_stats.first_noroute_ms),
            opt_ms(agent_stats.first_timeout_ms),
            opt_ms(agent_stats.first_refused_ms),
            opt_ms(agent_stats.first_other_ms),
            agent_stats.ok_count,
            agent_stats.hostdown_count,
            agent_stats.noroute_count,
            agent_stats.timeout_count,
            agent_stats.refused_count,
            agent_stats.other_count,
            opt_ms(resolve_ms),
            opt_ms(tcp_ms),
            opt_ms(handshake_ms),
            opt_ms(auth_ms),
            opt_ms(exec_ms),
            transitions.replace('\t', " "),
            note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;

        total_noroute += agent_stats.noroute_count;
        total_timeout += agent_stats.timeout_count;
        total_refused += agent_stats.refused_count;
        if let Some(ms) = agent_ready_ms {
            worst_agent_ready_ms = Some(worst_agent_ready_ms.map_or(ms, |old| old.max(ms)));
        }
        if let Some(ms) = ssh_ready_ms {
            worst_ssh_ready_ms = Some(worst_ssh_ready_ms.map_or(ms, |old| old.max(ms)));
        }

        let sample_recovered = down_ms.is_some()
            && agent_ready_ms.is_some()
            && ssh_ready_ms.is_some()
            && launcher_state == "LauncherActive"
            && agent_rx_nonzero
            && agent_rx_increasing;
        if sample_recovered {
            recovered += 1;
        } else if fail_on_timeout {
            return Err(format!(
                "agent boot-profile sample {sample}/{samples} failed mode={mode}: down_ms={} agent_ready_ms={} ssh_exec_ready_ms={} main_status_ms={} launcher_state={} note={}",
                opt_ms(down_ms),
                opt_ms(agent_ready_ms),
                opt_ms(ssh_ready_ms),
                opt_ms(main_status_ms),
                if launcher_state.is_empty() { "missing" } else { &launcher_state },
                note
            )
            .into());
        }
        thread::sleep(Duration::from_secs(2));
    }

    eprintln!(
        "agent boot-profile: {recovered}/{samples} {mode} reboots recovered; worst_agent_ready_ms={} worst_ssh_ready_ms={} noroute={} timeout={} refused={}",
        opt_ms(worst_agent_ready_ms),
        opt_ms(worst_ssh_ready_ms),
        total_noroute,
        total_timeout,
        total_refused
    );
    eprintln!("agent boot-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

fn boot_net_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 3);
    let reboot_mode = reboot_mode_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(120.0);
    let mode = reboot_mode.label();
    let out_path = "history/toolchain-bench/results-boot-net.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\ttcp22_ms\tssh_exec_ready_ms\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tmain_status_ms\tslint_status_ms\tuptime\tlauncher_state\tslint_frames\tnote";
    println!("{header}");
    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, reboot_mode)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            if !port_open(Duration::from_millis(200)) {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let recovery = measure_reboot_recovery(start, timeout_secs, reboot_note)?;

        let row = format!(
            "boot-net\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(recovery.tcp22_ms),
            opt_ms(recovery.ssh_ready_ms),
            opt_ms(recovery.resolve_ms),
            opt_ms(recovery.tcp_ms),
            opt_ms(recovery.handshake_ms),
            opt_ms(recovery.auth_ms),
            opt_ms(recovery.exec_ms),
            opt_ms(recovery.main_status_ms),
            opt_ms(recovery.slint_status_ms),
            recovery.uptime,
            recovery.launcher_state,
            recovery.slint_frames,
            recovery.note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;
        thread::sleep(Duration::from_secs(2));
    }
    eprintln!("boot-net-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RebootRecoveryMeasurement {
    tcp22_ms: Option<u128>,
    ssh_ready_ms: Option<u128>,
    resolve_ms: Option<u128>,
    tcp_ms: Option<u128>,
    handshake_ms: Option<u128>,
    auth_ms: Option<u128>,
    exec_ms: Option<u128>,
    main_status_ms: Option<u128>,
    slint_status_ms: Option<u128>,
    uptime: String,
    launcher_state: String,
    slint_frames: String,
    note: String,
}

fn measure_reboot_recovery(
    start: Instant,
    timeout_secs: f64,
    initial_note: String,
) -> Result<RebootRecoveryMeasurement> {
    let mut measurement = RebootRecoveryMeasurement {
        note: initial_note,
        ..Default::default()
    };
    while start.elapsed().as_secs_f64() < timeout_secs {
        if measurement.tcp22_ms.is_none() && port_open(Duration::from_millis(150)) {
            measurement.tcp22_ms = Some(start.elapsed().as_millis());
        }
        match connect_timed(2) {
            Ok(timed) => {
                let exec_t = Instant::now();
                let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                if out.rc != 0 {
                    measurement.note = format!("exec rc {}", out.rc);
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
                measurement.ssh_ready_ms = Some(start.elapsed().as_millis());
                measurement.resolve_ms = Some(timed.resolve_ms);
                measurement.tcp_ms = Some(timed.tcp_ms);
                measurement.handshake_ms = Some(timed.handshake_ms);
                measurement.auth_ms = Some(timed.auth_ms);
                measurement.exec_ms = Some(exec_t.elapsed().as_millis());
                measurement.uptime = out
                    .stdout
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let status_deadline = Instant::now() + Duration::from_secs(20);
                while Instant::now() < status_deadline {
                    update_reboot_status(&mut measurement, &timed.sess, start);
                    if measurement.main_status_ms.is_some() && measurement.slint_status_ms.is_some()
                    {
                        break;
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                break;
            }
            Err(error) => measurement.note = error.to_string(),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Ok(measurement)
}

fn update_reboot_status(
    measurement: &mut RebootRecoveryMeasurement,
    sess: &Session,
    start: Instant,
) {
    let main = remote_read(sess, MAIN_STATUS_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let slint = remote_read(sess, SLINT_STATUS_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    apply_reboot_status(
        measurement,
        main.as_ref(),
        slint.as_ref(),
        start.elapsed().as_millis(),
    );
}

fn apply_reboot_status(
    measurement: &mut RebootRecoveryMeasurement,
    main: Option<&Value>,
    slint: Option<&Value>,
    elapsed_ms: u128,
) {
    if measurement.main_status_ms.is_none()
        && let Some(value) = main
    {
        measurement.main_status_ms = Some(elapsed_ms);
        measurement.launcher_state = value
            .get("launcher_state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    if measurement.slint_status_ms.is_none()
        && let Some(value) = slint
    {
        measurement.slint_status_ms = Some(elapsed_ms);
        measurement.slint_frames = value
            .get("frames")
            .and_then(Value::as_u64)
            .map(|value| value.to_string())
            .unwrap_or_default();
    }
}

#[derive(Default)]
struct TcpProbeStats {
    ok_count: u64,
    hostdown_count: u64,
    noroute_count: u64,
    timeout_count: u64,
    refused_count: u64,
    other_count: u64,
    first_ok_ms: Option<u128>,
    first_hostdown_ms: Option<u128>,
    first_noroute_ms: Option<u128>,
    first_timeout_ms: Option<u128>,
    first_refused_ms: Option<u128>,
    first_other_ms: Option<u128>,
    last_label: String,
    transitions: Vec<String>,
}

impl TcpProbeStats {
    fn observe(&mut self, label: &str, elapsed_ms: u128) {
        match label {
            "ok" => {
                self.ok_count += 1;
                self.first_ok_ms.get_or_insert(elapsed_ms);
            }
            "hostdown" => {
                self.hostdown_count += 1;
                self.first_hostdown_ms.get_or_insert(elapsed_ms);
            }
            "noroute" => {
                self.noroute_count += 1;
                self.first_noroute_ms.get_or_insert(elapsed_ms);
            }
            "timeout" => {
                self.timeout_count += 1;
                self.first_timeout_ms.get_or_insert(elapsed_ms);
            }
            "refused" => {
                self.refused_count += 1;
                self.first_refused_ms.get_or_insert(elapsed_ms);
            }
            _ => {
                self.other_count += 1;
                self.first_other_ms.get_or_insert(elapsed_ms);
            }
        }

        if self.last_label != label {
            self.transitions.push(format!("{elapsed_ms}:{label}"));
            self.last_label = label.to_string();
        }
    }
}

fn boot_tcp_profile(args: &[String]) -> Result<()> {
    let samples = parse_profile_count(args, 1);
    let reboot_mode = reboot_mode_from_args(args)?;
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(40.0);
    let probe_timeout_ms = option_value(args, "--probe-timeout-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(100);
    let sleep_ms = option_value(args, "--sleep-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50);
    let mode = reboot_mode.label();
    let out_path = "history/toolchain-bench/results-boot-tcp.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\tfirst_ok_ms\tssh_exec_ready_ms\tfirst_hostdown_ms\tfirst_noroute_ms\tfirst_timeout_ms\tfirst_refused_ms\tfirst_other_ms\tok_count\thostdown_count\tnoroute_count\ttimeout_count\trefused_count\tother_count\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tuptime\ttransitions\tnote";
    println!("{header}");

    for sample in 1..=samples {
        let ts = unix_ms_now();
        let issue_t = Instant::now();
        let reboot_note = {
            let sess = connect(10)?;
            issue_reboot(&sess, reboot_mode)?
        };
        let reboot_issue_ms = issue_t.elapsed().as_millis();
        let start = Instant::now();
        let mut down_ms = None;
        while start.elapsed().as_secs_f64() < 40.0 {
            if !port_open(Duration::from_millis(200)) {
                down_ms = Some(start.elapsed().as_millis());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let mut stats = TcpProbeStats::default();
        let mut ssh_ready_ms = None;
        let mut resolve_ms = None;
        let mut tcp_ms = None;
        let mut handshake_ms = None;
        let mut auth_ms = None;
        let mut exec_ms = None;
        let mut uptime = String::new();
        let mut note = reboot_note;

        while start.elapsed().as_secs_f64() < timeout_secs {
            let elapsed_ms = start.elapsed().as_millis();
            let label = tcp_probe_label(Duration::from_millis(probe_timeout_ms));
            stats.observe(&label, elapsed_ms);
            if label == "ok" {
                break;
            }
            thread::sleep(Duration::from_millis(sleep_ms));
        }

        let ssh_deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < ssh_deadline {
            match connect_timed(2) {
                Ok(timed) => {
                    let exec_t = Instant::now();
                    let out = exec(&timed.sess, "cat /proc/uptime", true)?;
                    let this_exec_ms = exec_t.elapsed().as_millis();
                    if out.rc == 0 {
                        ssh_ready_ms = Some(start.elapsed().as_millis());
                        resolve_ms = Some(timed.resolve_ms);
                        tcp_ms = Some(timed.tcp_ms);
                        handshake_ms = Some(timed.handshake_ms);
                        auth_ms = Some(timed.auth_ms);
                        exec_ms = Some(this_exec_ms);
                        uptime = out
                            .stdout
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        break;
                    }
                    note = format!("exec rc {}", out.rc);
                }
                Err(err) => {
                    note = err.to_string();
                }
            }
            thread::sleep(Duration::from_millis(150));
        }

        let transitions = stats.transitions.join(",");
        let row = format!(
            "boot-tcp\t{ts}\t{sample}\t{mode}\t{}\t{reboot_issue_ms}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{uptime}\t{}\t{}",
            host(),
            opt_ms(down_ms),
            opt_ms(stats.first_ok_ms),
            opt_ms(ssh_ready_ms),
            opt_ms(stats.first_hostdown_ms),
            opt_ms(stats.first_noroute_ms),
            opt_ms(stats.first_timeout_ms),
            opt_ms(stats.first_refused_ms),
            opt_ms(stats.first_other_ms),
            stats.ok_count,
            stats.hostdown_count,
            stats.noroute_count,
            stats.timeout_count,
            stats.refused_count,
            stats.other_count,
            opt_ms(resolve_ms),
            opt_ms(tcp_ms),
            opt_ms(handshake_ms),
            opt_ms(auth_ms),
            opt_ms(exec_ms),
            transitions.replace('\t', " "),
            note.replace('\t', " ")
        );
        println!("{row}");
        append_profile_row(out_path, header, &row)?;
        thread::sleep(Duration::from_secs(2));
    }

    eprintln!("boot-tcp-profile: appended {samples} row(s) to {out_path}");
    Ok(())
}

fn watch_external_reboot(args: &[String]) -> Result<()> {
    let timeout_secs = option_value(args, "--timeout")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(120.0);
    let wait_down_secs = option_value(args, "--wait-down")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(180.0);
    let out_path = "history/toolchain-bench/results-boot-net.tsv";
    let header = "kind\tts_unix_ms\tsample\tmode\thost\treboot_issue_ms\tdown_ms\ttcp22_ms\tssh_exec_ready_ms\tresolve_ms\ttcp_ms\thandshake_ms\tauth_ms\texec_ms\tmain_status_ms\tslint_status_ms\tuptime\tlauncher_state\tslint_frames\tnote";
    println!("{header}");
    eprintln!(
        "watch-reboot: waiting up to {wait_down_secs:.0}s for {}:22 to go down...",
        host()
    );
    let ts = unix_ms_now();
    let wait_start = Instant::now();
    while wait_start.elapsed().as_secs_f64() < wait_down_secs {
        if !port_open(Duration::from_millis(200)) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if wait_start.elapsed().as_secs_f64() >= wait_down_secs {
        return Err(format!("device did not go down within {wait_down_secs:.0}s").into());
    }
    let start = Instant::now();
    eprintln!("watch-reboot: device went down; timing reconnect...");

    let recovery = measure_reboot_recovery(start, timeout_secs, String::from("external"))?;

    let row = format!(
        "boot-net\t{ts}\t1\texternal\t{}\t\t0\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        host(),
        opt_ms(recovery.tcp22_ms),
        opt_ms(recovery.ssh_ready_ms),
        opt_ms(recovery.resolve_ms),
        opt_ms(recovery.tcp_ms),
        opt_ms(recovery.handshake_ms),
        opt_ms(recovery.auth_ms),
        opt_ms(recovery.exec_ms),
        opt_ms(recovery.main_status_ms),
        opt_ms(recovery.slint_status_ms),
        recovery.uptime,
        recovery.launcher_state,
        recovery.slint_frames,
        recovery.note.replace('\t', " ")
    );
    println!("{row}");
    append_profile_row(out_path, header, &row)?;
    if recovery.ssh_ready_ms.is_some() {
        Ok(())
    } else {
        Err(format!("device not ready after {timeout_secs:.0}s").into())
    }
}

fn opt_ms(value: Option<u128>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentNetSnapshot {
    carrier: String,
    rx_packets: u64,
    tx_packets: u64,
}

fn agent_net_snapshot(value: &Value) -> Option<AgentNetSnapshot> {
    let result = value.get("result").unwrap_or(value);
    Some(AgentNetSnapshot {
        carrier: result
            .pointer("/network/carrier")
            .and_then(Value::as_str)?
            .to_string(),
        rx_packets: result
            .pointer("/network/stats/rx_packets")
            .and_then(Value::as_u64)?,
        tx_packets: result
            .pointer("/network/stats/tx_packets")
            .and_then(Value::as_u64)?,
    })
}

fn run_catalog_inspect(sess: &Session, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err(
            "usage: mister catalog (Catalog V3 validates the registry and every system shard)"
                .into(),
        );
    }
    let binary = configured_remote_path(
        "MISTER_MAGIK_BIN",
        "/media/fat/mister-magik/mister-magik-fb",
    );
    let command = remote_subcommand(&binary, "catalog-v3-inspect", args);
    let out = exec(sess, &command, true)?;
    print!("{}", out.stdout);
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    if let Some(error) = exec_failure_message("catalog inspect", &out) {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
fn parse_library_db_queries(args: &[String]) -> Result<(String, Vec<String>)> {
    let mut remote_path =
        configured_remote_path("MISTER_MAGIK_LIBRARY_DB", DEFAULT_REMOTE_LIBRARY_DB);
    let mut query_parts = Vec::new();
    let mut queries = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("db: --path needs a value".into());
                };
                remote_path = value.to_string();
                i += 2;
            }
            "--query" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("db: --query needs a statement".into());
                };
                queries.push(value.to_string());
                i += 2;
            }
            other => {
                query_parts.push(other.to_string());
                i += 1;
            }
        }
    }
    if !query_parts.is_empty() && !queries.is_empty() {
        return Err("db: cannot mix positional SQL with --query".into());
    }
    if !query_parts.is_empty() {
        queries.push(query_parts.join(" "));
    }
    if queries.is_empty() {
        return Err("usage: mister db [--path PATH] SQL | --query SQL [--query SQL ...]".into());
    }
    Ok((remote_path, queries))
}

fn remote_write(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(())
}

fn userspace_ready_fast_with(connection: &ConnectionConfig) -> Option<String> {
    let timed = connect_timed_with(connection, 2).ok()?;
    let out = exec(&timed.sess, "pidof MiSTer || echo BOOTING", true).ok()?;
    Some(out.stdout.trim().to_string())
}

fn wait_down(max_seconds: f64) -> bool {
    wait_down_with(&ConnectionConfig::from_environment(), max_seconds)
}

fn wait_down_with(connection: &ConnectionConfig, max_seconds: f64) -> bool {
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < max_seconds {
        if !port_open_with(connection, Duration::from_secs(2)) {
            println!(
                "  device went down after {:.1}s",
                start.elapsed().as_secs_f64()
            );
            return true;
        }
        thread::sleep(Duration::from_secs(1));
    }
    println!("  (device still answering; proceeding to wait-up anyway)");
    false
}

fn wait_up(max_seconds: f64) -> Result<i32> {
    wait_up_with(&ConnectionConfig::from_environment(), max_seconds)
}

fn wait_up_with(connection: &ConnectionConfig, max_seconds: f64) -> Result<i32> {
    let start = Instant::now();
    let mut attempt = 0;
    let mut last_print = Duration::MAX;
    while start.elapsed().as_secs_f64() < max_seconds {
        attempt += 1;
        let elapsed = start.elapsed().as_secs_f64();
        if port_open_with(connection, Duration::from_millis(150))
            && let Some(status) = userspace_ready_fast_with(connection)
        {
            let mister = if status == "BOOTING" {
                "booting".to_string()
            } else {
                format!("pid {status}")
            };
            println!(
                "SSH ready after {:.1}s (attempt {attempt}); MiSTer {mister}",
                start.elapsed().as_secs_f64()
            );
            return Ok(0);
        }
        if last_print == Duration::MAX || start.elapsed().saturating_sub(last_print).as_secs() >= 1
        {
            println!("  [{elapsed:5.1}s] waiting for ssh...");
            last_print = start.elapsed();
        }
        thread::sleep(Duration::from_millis(250));
    }
    println!("TIMEOUT: device not ready after {max_seconds:.0}s");
    println!("diagnostics: {}", host_wait_diagnostics_with(connection));
    Ok(1)
}

fn remote_read(sess: &Session, path: &str) -> Option<String> {
    let cmd = format!("cat {} 2>/dev/null", sh(path));
    let out = exec(sess, &cmd, true).ok()?;
    if out.rc == 0 { Some(out.stdout) } else { None }
}

fn remote_trim(sess: &Session, path: &str) -> Option<String> {
    remote_read(sess, path).map(|s| s.trim().to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum IniEdit {
    MagikBoot,
    MenuOutput(MenuOutputProfile),
    SelectMain(String),
    RestoreMain(Option<String>),
    RestoreDevelopmentMenu,
    ZaparooBoot,
    ArcadeVideo,
    MenuMode(String),
    StockBoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuOutputProfile {
    Hdmi,
    HdmiMode(&'static str),
    Auto,
    Crt240p60,
    Crt288p50,
    Crt480p60,
    Crt576p50,
}

impl MenuOutputProfile {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "hdmi" => Ok(Self::Hdmi),
            "1280x720p60" => Ok(Self::HdmiMode("0")),
            "1024x768p60" => Ok(Self::HdmiMode("1")),
            "720x480p60" => Ok(Self::HdmiMode("2")),
            "720x576p50" => Ok(Self::HdmiMode("3")),
            "1280x1024p60" => Ok(Self::HdmiMode("4")),
            "800x600p60" => Ok(Self::HdmiMode("5")),
            "640x480p60" => Ok(Self::HdmiMode("6")),
            "1280x720p50" => Ok(Self::HdmiMode("7")),
            "1920x1080p60" => Ok(Self::HdmiMode("8")),
            "1920x1080p50" => Ok(Self::HdmiMode("9")),
            "1366x768p60" => Ok(Self::HdmiMode("10")),
            "1024x600p60" => Ok(Self::HdmiMode("11")),
            "1920x1440p60" => Ok(Self::HdmiMode("12")),
            "2048x1536p60" => Ok(Self::HdmiMode("13")),
            "2560x1440p60" => Err("Mister does not support 1440p".into()),
            "auto" => Ok(Self::Auto),
            "crt-240p60" => Ok(Self::Crt240p60),
            "crt-288p50" => Ok(Self::Crt288p50),
            "crt-480p60" => Ok(Self::Crt480p60),
            "crt-576p50" => Ok(Self::Crt576p50),
            other => Err(format!("unsupported Menu output profile: {other}").into()),
        }
    }

    fn settings(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Hdmi | Self::HdmiMode(_) => ("0", "0", "0"),
            Self::Auto => ("2", "0", "0"),
            Self::Crt240p60 => ("1", "0", "0"),
            Self::Crt288p50 => ("1", "1", "0"),
            Self::Crt480p60 => ("1", "0", "1"),
            Self::Crt576p50 => ("1", "1", "1"),
        }
    }

    fn video_mode(self) -> Option<&'static str> {
        match self {
            Self::HdmiMode(mode) => Some(mode),
            _ => None,
        }
    }
}

fn parse_ini_edit_args(args: &[String]) -> Result<IniEdit> {
    validate_ini_edit_args(args)?;
    match args.first().map(String::as_str) {
        Some("menu") => Ok(IniEdit::MenuOutput(MenuOutputProfile::parse(&args[1])?)),
        Some("restore-development-menu") => Ok(IniEdit::RestoreDevelopmentMenu),
        Some("stock-boot") => Ok(IniEdit::StockBoot),
        _ => unreachable!("validated ini-edit arguments must parse"),
    }
}

fn validate_ini_edit_args(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("menu") if args.len() == 2 => {
            MenuOutputProfile::parse(&args[1])?;
        }
        Some("restore-development-menu") if args.len() == 1 => {}
        Some("stock-boot") if args.len() == 1 => {}
        Some("menu" | "restore-development-menu" | "stock-boot") => {
            return Err("ini-edit received the wrong number of arguments".into());
        }
        Some(other) => return Err(format!("unsupported ini-edit operation: {other}").into()),
        None => return Err("ini edit mode is required".into()),
    }
    Ok(())
}

fn edit_remote_ini(sess: &Session, edit: IniEdit, dry_run: bool) -> Result<()> {
    const INI: &str = "/media/fat/MiSTer.ini";
    let input = remote_read(sess, INI).ok_or("could not read /media/fat/MiSTer.ini")?;
    let edited = edit_mister_ini(&input, edit);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/media/fat/MiSTer.ini.mister-tool-new";
    remote_write(sess, tmp, edited.as_bytes())?;
    let out = exec(sess, &format!("mv {} {} && sync", sh(tmp), sh(INI)), true)?;
    if out.rc != 0 {
        return Err(format!("failed to replace {INI}: {}", out.stdout).into());
    }
    println!("MiSTer.ini edited with comment-preserving Rust mutator");
    Ok(())
}

fn ensure_stock_inittab(sess: &Session, dry_run: bool) -> Result<()> {
    const INITTAB: &str = "/etc/inittab";
    let input = remote_read(sess, INITTAB).ok_or("could not read /etc/inittab")?;
    let edited = ensure_stock_inittab_text(&input);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/tmp/inittab.mister-tool-new";
    remote_write(sess, tmp, edited.as_bytes())?;
    let out = exec(
        sess,
        &format!(
            "mount -o remount,rw / 2>/dev/null || true; cp {} {}; sync",
            sh(tmp),
            sh(INITTAB)
        ),
        true,
    )?;
    if out.rc != 0 {
        return Err(format!("failed to replace {INITTAB}: {}", out.stdout).into());
    }
    println!("inittab ensured -> stock MiSTer");
    Ok(())
}

fn ensure_stock_inittab_text(input: &str) -> String {
    let newline = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = Vec::new();
    let mut wrote = false;
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
            if !wrote {
                out.push("::sysinit:/media/fat/MiSTer &".to_string());
                wrote = true;
            }
            continue;
        }
        if line.starts_with("::sysinit:/media/fat/MiSTer_MagiK")
            || line.starts_with("::sysinit:/media/fat/mister-magik/boot.sh")
        {
            continue;
        }
        out.push(line.to_string());
    }
    if !wrote {
        out.push("::sysinit:/media/fat/MiSTer &".to_string());
    }
    let mut edited = out.join(newline);
    edited.push_str(newline);
    edited
}

fn edit_mister_ini(input: &str, edit: IniEdit) -> String {
    let mut document = mister_magik_ini::Document::parse(input.as_bytes())
        .expect("host-provided MiSTer.ini must be valid");

    match edit {
        IniEdit::MagikBoot => {
            document.set("MiSTer", "forced_scandoubler", "0");
            document.set("MiSTer", "menu_pal", "0");
            document.set("MiSTer", "direct_video", "2");
            document.set("MiSTer", "main", "MiSTer_MagiK");
            document.set("Menu", "video_mode", "8");
        }
        IniEdit::MenuOutput(profile) => {
            let (direct_video, menu_pal, forced_scandoubler) = profile.settings();
            document.set("Menu", "direct_video", direct_video);
            document.set("Menu", "menu_pal", menu_pal);
            document.set("Menu", "forced_scandoubler", forced_scandoubler);
            if let Some(video_mode) = profile.video_mode() {
                document.set("Menu", "video_mode", video_mode);
            }
        }
        IniEdit::SelectMain(value) => {
            document.set("MiSTer", "main", &value);
        }
        IniEdit::RestoreMain(value) => match value {
            Some(value) => document.set("MiSTer", "main", &value),
            None => document.remove(
                "MiSTer",
                "main",
                "MiSTer MagiK alpha acceptance restored absent value",
            ),
        },
        IniEdit::RestoreDevelopmentMenu => {
            document.set("MiSTer", "main", "MiSTer_MagiKDev");
            document.remove(
                "Menu",
                "video_mode",
                "MiSTer MagiK benchmark override removed",
            );
            document.remove(
                "Menu",
                "direct_video",
                "MiSTer MagiK benchmark override removed",
            );
        }
        IniEdit::ZaparooBoot => {
            document.set("MiSTer", "direct_video", "0");
            document.set("MiSTer", "main", "zaparoo/MiSTer_Zaparoo");
            document.set("Menu", "direct_video", "0");
            document.set("Menu", "video_mode", "8");
        }
        IniEdit::ArcadeVideo => {
            document.set("MiSTer", "direct_video", "0");
            document.set("arcade", "direct_video", "1");
            document.set("arcade_vertical", "direct_video", "0");
            document.set("arcade_vertical", "video_mode", "8");
            document.set("arcade_vertical", "vscale_mode", "1");
            document.ensure_section_after("arcade", "arcade_vertical");
        }
        IniEdit::MenuMode(mode) => {
            document.set("Menu", "video_mode", &mode);
        }
        IniEdit::StockBoot => {
            document.comment_if_value(
                "MiSTer",
                "main",
                &["MiSTer_MagiK", "MiSTer_MagiKDev", "mister-magik-fb"],
                "MiSTer MagiK stock boot restore",
            );
        }
    }

    String::from_utf8(document.render()).expect("MiSTer.ini renderer emits UTF-8")
}

fn collect_status(sess: &Session) -> Result<Value> {
    let main_status = parse_json(remote_read(sess, "/tmp/mister-magik/main-status.json"));
    let slint_status = parse_json(remote_read(sess, "/tmp/mister-magik/status.json"));
    let owner = main_status
        .as_ref()
        .and_then(|v| v.get("visible_owner"))
        .and_then(Value::as_str);
    let visual = json!({
        "class": "not_sampled",
        "note": "Use mister --capture-buffer for an agent-backed PNG capture."
    });
    let fb0_visible_candidate = owner == Some("fb0");
    Ok(json!({
        "schema": "mister-magik-status-v1",
        "collected_at_unix": unix_secs(),
        "device": {
            "hostname": remote_trim(sess, "/proc/sys/kernel/hostname"),
            "uptime": remote_trim(sess, "/proc/uptime"),
            "arch": exec_stdout(sess, "uname -m")?.trim(),
        },
        "processes": {
            "MiSTer": process_list(sess, "MiSTer")?,
            "MiSTer_MagiK": process_list(sess, "MiSTer_MagiK")?,
            "MiSTer_MagiKDev": process_list(sess, "MiSTer_MagiKDev")?,
            "mister-magik-fb": process_list(sess, "mister-magik-fb")?,
        },
        "boot": {
            "ini_keys": parse_ini_keys(remote_read(sess, "/media/fat/MiSTer.ini").unwrap_or_default()),
            "inittab": lines_containing(remote_read(sess, "/etc/inittab").unwrap_or_default(), &["MiSTer", "mister-magik"]),
        },
        "display": {
            "proc_fb": remote_trim(sess, "/proc/fb"),
            "fb_mode": remote_trim(sess, "/sys/module/MiSTer_fb/parameters/mode"),
            "virtual_size": remote_trim(sess, "/sys/class/graphics/fb0/virtual_size"),
            "bits_per_pixel": remote_trim(sess, "/sys/class/graphics/fb0/bits_per_pixel"),
            "stride": remote_trim(sess, "/sys/class/graphics/fb0/stride"),
            "active_vt": remote_trim(sess, "/sys/class/tty/tty0/active"),
            "fb0_visual": visual,
            "fb0_visible_candidate": fb0_visible_candidate,
        },
        "runtime": {
            "slint_status": slint_status,
            "main_status": main_status,
            "events_tail": tail_remote(sess, "/tmp/mister-magik/events.jsonl", 30),
            "logs": {
                "main": tail_remote(sess, "/tmp/mister-magik-main.log", 20),
                "slint": tail_remote(sess, "/tmp/mister-magik-slint.log", 20),
            }
        },
        "input": {
            "devices": parse_input_devices(remote_read(sess, "/proc/bus/input/devices").unwrap_or_default()),
        },
        "owners": fd_owners(sess)?,
        "audio": {
            "mr_audio_exists": exec(sess, "[ -e /dev/MrAudio ]", true)?.rc == 0,
        }
    }))
}

fn parse_json(text: Option<String>) -> Option<Value> {
    text.and_then(|s| serde_json::from_str(&s).ok())
}

fn exec_stdout(sess: &Session, cmd: &str) -> Result<String> {
    Ok(exec(sess, cmd, true)?.stdout)
}

fn process_list(sess: &Session, name: &str) -> Result<Vec<Value>> {
    let pids = exec_stdout(sess, &format!("pidof {} 2>/dev/null || true", sh(name)))?;
    let mut out = Vec::new();
    for pid in pids
        .split_whitespace()
        .filter_map(|s| s.parse::<u32>().ok())
    {
        let status = remote_read(sess, &format!("/proc/{pid}/status")).unwrap_or_default();
        let mut item = serde_json::Map::new();
        item.insert("pid".to_string(), json!(pid));
        for line in status.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            if matches!(
                k,
                "Name" | "State" | "PPid" | "VmRSS" | "Threads" | "Cpus_allowed_list"
            ) {
                item.insert(k.to_ascii_lowercase(), json!(v.trim()));
            }
        }
        item.insert("pid".to_string(), json!(pid));
        let cmd = exec_stdout(
            sess,
            &format!("tr '\\0' ' ' < /proc/{pid}/cmdline 2>/dev/null || true"),
        )?;
        item.insert("cmdline".to_string(), json!(cmd.trim()));
        out.push(Value::Object(item));
    }
    Ok(out)
}

fn parse_ini_keys(text: String) -> Value {
    let mut root = serde_json::Map::new();
    let mut section = "global".to_string();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.contains(']') {
            section = line[1..line.find(']').unwrap()].to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if matches!(
            key,
            "main"
                | "video_mode"
                | "direct_video"
                | "menu_pal"
                | "forced_scandoubler"
                | "fb_terminal"
                | "fb_size"
        ) {
            let sec = root.entry(section.clone()).or_insert_with(|| json!({}));
            sec.as_object_mut().unwrap().insert(
                key.to_string(),
                json!({"value": value.trim(), "line": idx + 1}),
            );
        }
    }
    Value::Object(root)
}

fn lines_containing(text: String, needles: &[&str]) -> Vec<String> {
    text.lines()
        .filter(|line| needles.iter().any(|n| line.contains(n)))
        .map(ToString::to_string)
        .collect()
}

fn tail_remote(sess: &Session, path: &str, n: usize) -> Option<Vec<String>> {
    let out = exec(sess, &format!("tail -n {n} {} 2>/dev/null", sh(path)), true).ok()?;
    if out.rc == 0 {
        Some(out.stdout.lines().map(ToString::to_string).collect())
    } else {
        None
    }
}

fn parse_input_devices(text: String) -> Vec<Value> {
    let mut out = Vec::new();
    let mut current = serde_json::Map::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(Value::Object(std::mem::take(&mut current)));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("N: Name=") {
            current.insert("name".to_string(), json!(rest.trim().trim_matches('"')));
        } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
            current.insert(
                "handlers".to_string(),
                json!(rest.split_whitespace().collect::<Vec<_>>()),
            );
        } else if let Some(rest) = line.strip_prefix("I: ") {
            current.insert("id".to_string(), json!(rest.trim()));
        }
    }
    if !current.is_empty() {
        out.push(Value::Object(current));
    }
    out
}

fn fd_owners(sess: &Session) -> Result<Value> {
    let script = r#"
for name in MiSTer MiSTer_MagiK MiSTer_MagiKDev mister-magik-fb; do
  for p in $(pidof "$name" 2>/dev/null); do
    for fd in /proc/$p/fd/*; do
      t=$(readlink "$fd" 2>/dev/null || true)
      case "$t" in
        /dev/fb0|/dev/mem|/dev/tty0|/dev/tty2|/dev/MiSTer_cmd|/dev/MrAudio|/dev/uinput|/dev/input/*)
          echo "$p	$name	${fd##*/}	$t"
          ;;
      esac
    done
  done
done
"#;
    let rows = exec_stdout(sess, script)?;
    let mut by_device = serde_json::Map::new();
    let mut by_process = serde_json::Map::new();
    for line in rows.lines() {
        let parts: Vec<_> = line.split('\t').collect();
        if parts.len() != 4 {
            continue;
        }
        let pid = parts[0].parse::<u32>().unwrap_or(0);
        let fd = parts[2].parse::<u32>().unwrap_or(0);
        let proc_item = json!({"pid": pid, "process": parts[1], "fd": fd, "target": parts[3]});
        by_device
            .entry(parts[3].to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!({"pid": pid, "process": parts[1], "fd": fd}));
        by_process
            .entry(parts[0].to_string())
            .or_insert_with(|| json!({"process": parts[1], "fds": []}))
            .get_mut("fds")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(proc_item);
    }
    Ok(json!({"by_device": by_device, "by_process": by_process}))
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FbGeometry {
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
}

#[cfg(test)]
impl FbGeometry {
    fn bytes(self) -> Result<usize> {
        self.stride
            .checked_mul(self.height)
            .ok_or_else(|| "framebuffer byte size overflow".into())
    }
}

#[cfg(test)]
fn parse_virtual_size(text: &str) -> Option<(usize, usize)> {
    let (w, h) = text.trim().split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[cfg(test)]
fn classify_fb(raw: &[u8], geometry: &FbGeometry) -> Value {
    let mut samples = 0u32;
    let mut nonzero = 0u32;
    let mut blackish = 0u32;
    let mut transitions = 0u32;
    let mut color_min = 0x00ff_ffffu32;
    let mut color_max = 0u32;
    let mut prev = None;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for y in (0..geometry.height).step_by(16) {
        for x in (0..geometry.width).step_by(16) {
            let Some((r, g, b)) = rgb_from_raw(raw, geometry, x, y) else {
                continue;
            };
            let p = (r << 16) | (g << 8) | b;
            samples += 1;
            nonzero += u32::from(p != 0);
            blackish += u32::from(r < 8 && g < 8 && b < 8);
            color_min = color_min.min(p);
            color_max = color_max.max(p);
            if let Some(prev) = prev
                && color_distance(prev, p) > 96
            {
                transitions += 1;
            }
            prev = Some(p);
            hash ^= p as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    let nonzero_pct = pct(nonzero, samples);
    let blackish_pct = pct(blackish, samples);
    let transition_pct = pct(transitions, samples.saturating_sub(1).max(1));
    let class = if blackish_pct >= 95.0 {
        "mostly_black"
    } else if nonzero_pct >= 20.0 && transition_pct >= 35.0 {
        "static_like"
    } else if nonzero_pct >= 5.0 {
        "slint_like"
    } else {
        "unknown"
    };
    json!({
        "ok": true,
        "width": geometry.width,
        "height": geometry.height,
        "stride": geometry.stride,
        "bpp": geometry.bpp,
        "step": 16,
        "samples": samples,
        "nonzero": nonzero,
        "blackish": blackish,
        "transitions": transitions,
        "nonzero_pct": round2(nonzero_pct),
        "blackish_pct": round2(blackish_pct),
        "transition_pct": round2(transition_pct),
        "color_min": format!("{color_min:06x}"),
        "color_max": format!("{color_max:06x}"),
        "class": class,
        "hash": format!("{hash:016x}"),
    })
}

#[cfg(test)]
fn color_distance(a: u32, b: u32) -> u32 {
    let ar = (a >> 16) & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = a & 0xff;
    let br = (b >> 16) & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = b & 0xff;
    ar.abs_diff(br) + ag.abs_diff(bg) + ab.abs_diff(bb)
}

#[cfg(test)]
fn pct(n: u32, d: u32) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 * 100.0 / d as f64
    }
}

#[cfg(test)]
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn print_status_summary(status: &Value) {
    let display = &status["display"];
    let visual = &display["fb0_visual"];
    println!("MiSTer status");
    println!(
        "  active_vt: {}",
        display["active_vt"].as_str().unwrap_or("?")
    );
    println!(
        "  fb_mode:   {}",
        display["fb_mode"].as_str().unwrap_or("?")
    );
    println!(
        "  fb0:       {} hash={}",
        visual["class"].as_str().unwrap_or("unknown"),
        visual["hash"].as_str().unwrap_or("?")
    );
    println!(
        "  boot:      [MiSTer] main={} [Menu] direct_video={} menu_pal={} forced_scandoubler={} video_mode={}",
        ini_value(status, "MiSTer", "main").unwrap_or("?"),
        ini_value(status, "Menu", "direct_video").unwrap_or("?"),
        ini_value(status, "Menu", "menu_pal").unwrap_or("?"),
        ini_value(status, "Menu", "forced_scandoubler").unwrap_or("?"),
        ini_value(status, "Menu", "video_mode").unwrap_or("?")
    );
    println!(
        "  arcade:   [arcade] direct_video={} [arcade_vertical] direct_video={} video_mode={}",
        ini_value(status, "arcade", "direct_video").unwrap_or("?"),
        ini_value(status, "arcade_vertical", "direct_video").unwrap_or("?"),
        ini_value(status, "arcade_vertical", "video_mode").unwrap_or("?")
    );
    for name in [
        "MiSTer",
        "MiSTer_MagiK",
        "MiSTer_MagiKDev",
        "mister-magik-fb",
    ] {
        let pid = primary_process(status, name)
            .and_then(|v| v["pid"].as_u64())
            .map(|p| p.to_string())
            .unwrap_or_else(|| "none".to_string());
        println!("  {name:<15} pid={pid}");
    }
    let magik_pids = process_pids(status, "mister-magik-fb");
    if magik_pids.len() > 1 {
        println!("  mister-magik-fb extra_pids={}", format_pids(&magik_pids));
    }
    if let Some(main) = status["runtime"]["main_status"].as_object() {
        println!(
            "  main:      state={} launcher_pid={} ready={} attempt={} remaining_ms={} last_failure={} visible_owner={}",
            main.get("launcher_state")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            main.get("launcher_pid")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            main.get("launcher_ready_phase")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            main.get("launcher_ready_attempt")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            main.get("launcher_ready_remaining_ms")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            main.get("launcher_ready_last_failure")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            main.get("visible_owner")
                .and_then(Value::as_str)
                .unwrap_or("?")
        );
    }
    if let Some(slint) = status["runtime"]["slint_status"].as_object() {
        println!(
            "  slint:     scene={} screen={} fps={} frames={}",
            slint.get("scene").and_then(Value::as_str).unwrap_or("?"),
            slint.get("screen").and_then(Value::as_str).unwrap_or("?"),
            slint
                .get("fps_estimate")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            slint
                .get("frames")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into())
        );
    }
}

fn primary_process<'a>(status: &'a Value, name: &str) -> Option<&'a Value> {
    let processes = status["processes"][name].as_array()?;
    if name == "mister-magik-fb" {
        processes
            .iter()
            .find(|process| {
                process["cmdline"]
                    .as_str()
                    .is_some_and(|cmd| cmd.contains(" ui launcher "))
            })
            .or_else(|| processes.first())
    } else {
        processes.first()
    }
}

fn process_pids(status: &Value, name: &str) -> Vec<u64> {
    status["processes"][name]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|process| process["pid"].as_u64())
        .collect()
}

fn format_pids(pids: &[u64]) -> String {
    pids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn ini_value<'a>(status: &'a Value, section: &str, key: &str) -> Option<&'a str> {
    status["boot"]["ini_keys"][section][key]["value"].as_str()
}

fn ini_line(status: &Value, section: &str, key: &str) -> Option<u64> {
    status["boot"]["ini_keys"][section][key]["line"].as_u64()
}

fn doctor_findings(status: &Value) -> Vec<(String, String)> {
    let mut findings = Vec::new();
    let expected_main =
        std::env::var("MISTER_MAGIK_MAIN_NAME").unwrap_or_else(|_| "MiSTer_MagiK".to_string());
    if ini_value(status, "MiSTer", "main") != Some(expected_main.as_str()) {
        findings.push((
            "error".into(),
            format!("[MiSTer] main is not {expected_main}"),
        ));
    }
    if !matches!(
        ini_value(status, "Menu", "direct_video"),
        Some("0" | "1" | "2")
    ) {
        findings.push((
            "warn".into(),
            "[Menu] direct_video is not HDMI (0), CRT (1), or automatic (2)".into(),
        ));
    }
    for key in ["menu_pal", "forced_scandoubler"] {
        if !matches!(ini_value(status, "Menu", key), Some("0" | "1")) {
            findings.push(("warn".into(), format!("[Menu] {key} is not 0 or 1")));
        }
    }
    if ini_value(status, "arcade", "direct_video") != Some("1") {
        findings.push((
            "warn".into(),
            "[arcade] direct_video is not 1; normal arcade games will use scaler output".into(),
        ));
    }
    if ini_value(status, "arcade_vertical", "direct_video") != Some("0") {
        findings.push((
            "warn".into(),
            "[arcade_vertical] direct_video is not 0; rotated games may bypass MiSTer rotation"
                .into(),
        ));
    }
    if ini_value(status, "arcade_vertical", "video_mode") != Some("8") {
        findings.push((
            "warn".into(),
            "[arcade_vertical] video_mode is not 8; rotated games should use 1080p scaler mode"
                .into(),
        ));
    }
    if let (Some(arcade), Some(vertical)) = (
        ini_line(status, "arcade", "direct_video"),
        ini_line(status, "arcade_vertical", "direct_video"),
    ) && arcade > vertical
    {
        findings.push((
            "warn".into(),
            "[arcade] appears after [arcade_vertical]; vertical arcade settings will be overwritten"
                .into(),
        ));
    }
    for name in [expected_main.as_str(), "mister-magik-fb"] {
        if status["processes"][name]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
        {
            findings.push(("error".into(), format!("{name} is not running")));
        }
    }
    if status["display"]["active_vt"].as_str() != Some("tty2") {
        findings.push((
            "warn".into(),
            format!(
                "active VT is {}, expected tty2 for launcher",
                status["display"]["active_vt"].as_str().unwrap_or("?")
            ),
        ));
    }
    match status["display"]["fb0_visual"]["class"].as_str() {
        Some("mostly_black") => {
            findings.push(("error".into(), "/dev/fb0 samples as mostly_black".into()))
        }
        Some("not_sampled") => {}
        Some("unknown") | None => {
            findings.push(("warn".into(), "/dev/fb0 visual class is unknown".into()))
        }
        _ => {}
    }
    if let Some(owner) = status["runtime"]["main_status"]["visible_owner"].as_str()
        && owner != "fb0"
    {
        findings.push((
            "warn".into(),
            format!("Main reports visible_owner={owner} rather than fb0"),
        ));
    }
    let main = &status["runtime"]["main_status"];
    if main["launcher_state"].as_str() == Some("LauncherStarting") {
        findings.push((
            "warn".into(),
            format!(
                "launcher readiness is still {} (attempt {}, {}ms remaining; last failure {})",
                main["launcher_ready_phase"].as_str().unwrap_or("unknown"),
                main["launcher_ready_attempt"].as_u64().unwrap_or(0),
                main["launcher_ready_remaining_ms"].as_u64().unwrap_or(0),
                main["launcher_ready_last_failure"]
                    .as_str()
                    .unwrap_or("none")
            ),
        ));
    } else if main["launcher_state"].as_str() == Some("Unconfigured")
        && main["launcher_ready_last_failure"]
            .as_str()
            .is_some_and(|failure| failure != "none")
    {
        findings.push((
            "warn".into(),
            format!(
                "launcher readiness fallback restored stock Menu after {}",
                main["launcher_ready_last_failure"]
                    .as_str()
                    .unwrap_or("unknown failure")
            ),
        ));
    }
    let fb_owned = status["owners"]["by_device"]["/dev/fb0"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|o| o["process"].as_str() == Some("mister-magik-fb"))
        })
        .unwrap_or(false);
    if !fb_owned {
        findings.push((
            "warn".into(),
            "/dev/fb0 is not owned by mister-magik-fb".into(),
        ));
    }
    let magik_fb0_owner_pids = magik_fb0_owner_pids(status);
    if magik_fb0_owner_pids.len() > 1 {
        findings.push((
            "error".into(),
            format!(
                "multiple mister-magik-fb processes own /dev/fb0: {}",
                format_pids(&magik_fb0_owner_pids)
            ),
        ));
    }
    if findings.is_empty() {
        findings.push((
            "ok".into(),
            "No obvious launcher/display problems found".into(),
        ));
    }
    findings
}

fn magik_fb0_owner_pids(status: &Value) -> Vec<u64> {
    let mut pids = Vec::new();
    if let Some(items) = status["owners"]["by_device"]["/dev/fb0"].as_array() {
        for item in items {
            if item["process"].as_str() == Some("mister-magik-fb")
                && let Some(pid) = item["pid"].as_u64()
                && !pids.contains(&pid)
            {
                pids.push(pid);
            }
        }
    }
    pids
}

fn boot_capture(deploy: bool, keep_enabled: bool, settle_secs: u64) -> Result<()> {
    if deploy {
        return Err("boot-capture --deploy is retired; commit the platform change and run scripts/agent deliver first".into());
    }
    {
        let sess = connect(10)?;
        let app = configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik");
        let command = format!(
            "mkdir -p {0}; : > {0}/boot-analytics.enabled; sync",
            sh(&app)
        );
        let _ = exec(&sess, &command, true)?;
        let issued = issue_reboot(&sess, RebootMode::Supervised)?;
        println!("reboot issued to {} ({issued})", host());
    }
    wait_down(40.0);
    if wait_up(120.0)? != 0 {
        return Err("device did not return after reboot".into());
    }
    thread::sleep(Duration::from_secs(settle_secs));
    let sess = connect(10)?;
    let dir = PathBuf::from("build/boot-analytics").join(timestamp());
    fs::create_dir_all(&dir)?;
    let status = collect_status(&sess)?;
    fs::write(dir.join("status.json"), serde_json::to_vec_pretty(&status)?)?;
    for (remote, local) in [
        ("/tmp/mister-magik-boot-analytics.tsv", "boot-analytics.tsv"),
        ("/tmp/mister-magik/events.jsonl", "events.jsonl"),
        ("/tmp/mister-magik/status.json", "slint-status.json"),
        ("/tmp/mister-magik/main-status.json", "main-status.json"),
        ("/tmp/mister-magik-slint.log", "slint.log"),
        ("/tmp/mister-magik-main.log", "main.log"),
        (
            "/tmp/mister-magik-launcher-frame-profile.tsv",
            "launcher-frame-profile.tsv",
        ),
        ("/tmp/mister-magik-visual-samples.tsv", "visual-samples.tsv"),
    ] {
        if get(&sess, remote, &dir.join(local)).is_err() {
            fs::write(dir.join(format!("{local}.missing")), remote)?;
        }
    }
    if !keep_enabled {
        let app = configured_remote_path("MISTER_MAGIK_APP_DIR", "/media/fat/mister-magik");
        let _ = exec(
            &sess,
            &format!("rm -f {}/boot-analytics.enabled; sync", sh(&app)),
            true,
        );
    }
    println!("boot-capture: {}", dir.display());
    Ok(())
}

fn display_read(sess: &Session, unsafe_spi: bool, json_out: bool) -> Result<()> {
    let status = collect_status(sess)?;
    if display_read_needs_unsafe_spi(&status) && !unsafe_spi {
        return Err(
            "display-read touches FPGA SPI; pass --unsafe-spi when Main/Slint may own /dev/mem"
                .into(),
        );
    }
    let binary = configured_remote_path(
        "MISTER_MAGIK_BIN",
        "/media/fat/mister-magik/mister-magik-fb",
    );
    let out = exec(sess, &format!("{} read", sh(&binary)), true)?;
    if json_out {
        println!("{}", json!({"rc": out.rc, "output": out.stdout}));
    } else {
        print!("{}", out.stdout);
    }
    std::process::exit(out.rc);
}

fn display_read_needs_unsafe_spi(status: &Value) -> bool {
    ["MiSTer_MagiKDev", "MiSTer_MagiK", "MiSTer"]
        .iter()
        .any(|name| {
            status["processes"][name]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        })
}

fn profile_summary(path: &Path) -> Result<()> {
    print!("{}", profile_summary_text(path)?);
    Ok(())
}

fn profile_summary_text(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header: Vec<_> = lines.next().ok_or("empty TSV")?.split('\t').collect();
    let rows: Vec<Vec<_>> = lines.map(|l| l.split('\t').collect()).collect();
    let mut out = String::new();
    out.push_str(&format!(
        "=== {} ({} frames) ===\n",
        path.display(),
        rows.len()
    ));
    for col in [
        "wall_us",
        "phases_us",
        "anim_us",
        "render_us",
        "vsync_us",
        "copy_us",
    ] {
        let Some(idx) = header.iter().position(|h| *h == col) else {
            continue;
        };
        let mut vals: Vec<u64> = rows
            .iter()
            .filter_map(|r| r.get(idx).and_then(|v| v.parse().ok()))
            .collect();
        if vals.is_empty() {
            continue;
        }
        vals.sort_unstable();
        let avg = vals.iter().sum::<u64>() / vals.len() as u64;
        let p50 = vals[vals.len() / 2];
        let p95 = vals[((vals.len() - 1) as f64 * 0.95) as usize];
        out.push_str(&format!(
            "{col:10} min={:6} p50={p50:6} p95={p95:6} max={:6} avg={avg:6}",
            vals[0],
            vals[vals.len() - 1]
        ));
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
fn rgb_from_raw(raw: &[u8], geometry: &FbGeometry, x: usize, y: usize) -> Option<(u32, u32, u32)> {
    match geometry.bpp {
        32 => {
            let i = y
                .checked_mul(geometry.stride)?
                .checked_add(x.checked_mul(4)?)?;
            if i + 2 >= raw.len() {
                return None;
            }
            Some((raw[i + 2] as u32, raw[i + 1] as u32, raw[i] as u32))
        }
        16 => {
            let i = y
                .checked_mul(geometry.stride)?
                .checked_add(x.checked_mul(2)?)?;
            if i + 1 >= raw.len() {
                return None;
            }
            let v = u16::from_le_bytes([raw[i], raw[i + 1]]);
            let r5 = (v >> 11) & 0x1f;
            let g6 = (v >> 5) & 0x3f;
            let b5 = v & 0x1f;
            let r = ((r5 << 3) | (r5 >> 2)) as u32;
            let g = ((g6 << 2) | (g6 >> 4)) as u32;
            let b = ((b5 << 3) | (b5 >> 2)) as u32;
            Some((r, g, b))
        }
        _ => None,
    }
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name && !looks_like_option_token(&pair[1]))
        .map(|pair| pair[1].clone())
}

fn option_values(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .filter(|pair| !looks_like_option_token(&pair[1]))
        .map(|pair| pair[1].clone())
        .collect()
}

fn looks_like_option_token(value: &str) -> bool {
    value.starts_with("--")
        || value
            .strip_prefix('-')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphabetic())
}

fn timestamp() -> String {
    unix_secs().to_string()
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_return_cleanup_does_not_restart_main_during_game_handoff() {
        assert!(launch_return_cleanup_needs_active_restart(&json!({
            "launcher_state": "LauncherActive"
        })));
        for state in ["Unconfigured", "HandoffToGame", "EnteringLauncher"] {
            assert!(!launch_return_cleanup_needs_active_restart(&json!({
                "launcher_state": state
            })));
        }
    }

    #[test]
    fn native_device_config_retains_resolved_identity_and_forwards_agent_state() {
        let connection =
            ConnectionConfig::from_values("192.0.2.5", Some("operator"), Some("credential"));
        let config =
            NativeDeviceConfig::new(connection.clone(), "device-id".into(), "token-value".into());

        assert_eq!(config.connection, connection);
        assert_eq!(config.device_id, "device-id");
        assert_eq!(config.agent, AgentEndpoint::new("192.0.2.5", "token-value"));
    }

    #[test]
    fn display_matrix_covers_every_supported_runtime_mode() {
        assert_eq!(DISPLAY_MATRIX_MODES.len(), 11);
        assert!(DISPLAY_MATRIX_MODES.iter().any(|mode| mode.id == "auto"));
        assert!(
            DISPLAY_MATRIX_MODES
                .iter()
                .any(|mode| mode.id == "crt-576p50")
        );
        assert!(
            release_display_mode_command_for_runtime().contains("latch-readiness-report --json")
        );
    }

    #[test]
    fn display_matrix_args_enable_usb_video_without_exposing_credentials() {
        let args = [
            "--attended",
            "--out",
            "/tmp/evidence",
            "--usb-video",
            "--screensaver-wait",
            "65",
        ]
        .map(str::to_string);
        assert_eq!(
            parse_display_matrix_args(&args).unwrap(),
            ("/tmp/evidence", true, Some(65))
        );
        let args = ["--attended", "--out", "/tmp/evidence"].map(str::to_string);
        assert_eq!(
            parse_display_matrix_args(&args).unwrap(),
            ("/tmp/evidence", false, None)
        );
        let duplicate = [
            "--attended",
            "--out",
            "/tmp/evidence",
            "--usb-video",
            "--usb-video",
        ]
        .map(str::to_string);
        assert!(parse_display_matrix_args(&duplicate).is_err());
    }

    #[test]
    fn display_matrix_wait_honors_interruption() {
        DISPLAY_MATRIX_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(wait_display_matrix_interval(Duration::from_secs(1)).is_err());
        DISPLAY_MATRIX_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn single_display_mode_args_require_attended_and_parse_keep() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        };
        let (mode, keep) =
            parse_display_mode_args(&args(&["hdmi-1280x720p60", "--attended"])).unwrap();
        assert_eq!(mode.id, "hdmi-1280x720p60");
        assert!(!keep);
        assert!(
            parse_display_mode_args(&args(&["hdmi-1920x1080p60", "--attended", "--keep",]))
                .unwrap()
                .1
        );
        assert!(parse_display_mode_args(&args(&["hdmi-1280x720p60"])).is_err());
        assert!(parse_display_mode_args(&args(&["unsafe", "--attended"])).is_err());
        assert!(
            parse_display_mode_args(&args(&["hdmi-1280x720p60", "--attended", "--other",]))
                .is_err()
        );
    }

    #[test]
    fn display_matrix_readiness_records_geometry_frames_and_idle_state() {
        let parsed = parse_display_matrix_readiness(
            "plan\tdisplay-plan: output=640x240 scan=640x240 fb=640x240\nframes\t10\t12\nidle\tfalse\n",
        )
        .unwrap();
        assert_eq!(
            parsed,
            DisplayMatrixReadiness {
                output: (640, 240),
                framebuffer: (640, 240),
                frames_before: 10,
                frames_after: 12,
                idle: false,
            }
        );
        assert!(parse_display_matrix_readiness("frames\t10\t12\n").is_err());
    }

    #[test]
    fn display_matrix_geometry_requires_native_crt_framebuffers() {
        for (id, height, retired_framebuffer) in [
            ("crt-240p60", 240, (320, 240)),
            ("crt-288p50", 288, (384, 288)),
            ("crt-480p60", 480, (320, 480)),
            ("crt-576p50", 576, (640, 480)),
        ] {
            let mode = DISPLAY_MATRIX_MODES
                .iter()
                .find(|mode| mode.id == id)
                .copied()
                .unwrap();
            assert!(validate_display_matrix_geometry(mode, (640, height), (640, height)).is_ok());
            assert!(
                validate_display_matrix_geometry(mode, (640, height), retired_framebuffer).is_err()
            );
        }

        let hdmi = DISPLAY_MATRIX_MODES
            .iter()
            .find(|mode| mode.id == "hdmi-1366x768p60")
            .copied()
            .unwrap();
        assert!(validate_display_matrix_geometry(hdmi, (1366, 768), (683, 384)).is_ok());
        assert!(validate_display_matrix_geometry(hdmi, (1366, 768), (1366, 768)).is_err());
    }

    #[test]
    fn display_confirmation_status_handles_success_failure_timeout_and_interrupt() {
        assert!(
            display_transaction_complete(
                "ok DisplayV1 schema=1 active=hdmi-1280x720p60 pending=none phase=idle",
                false,
            )
            .unwrap()
        );
        assert!(!display_transaction_complete(
            "ok DisplayV1 schema=1 active=hdmi-1920x1080p60 pending=hdmi-1280x720p60 phase=persisting",
            false,
        )
        .unwrap());
        assert!(display_transaction_complete(
            "ok DisplayV1 schema=1 active=hdmi-1920x1080p60 pending=hdmi-1280x720p60 phase=failed",
            false,
        )
        .is_err());
        assert!(display_transaction_complete(
            "ok DisplayV1 schema=1 active=hdmi-1920x1080p60 pending=hdmi-1280x720p60 phase=persisting",
            true,
        )
        .is_err());
        assert_eq!(
            display_mode_completion_action(true, false).unwrap(),
            DisplayModeCompletionAction::Confirm
        );
        assert_eq!(
            display_mode_completion_action(false, false).unwrap(),
            DisplayModeCompletionAction::Rollback
        );
        assert!(display_mode_completion_action(true, true).is_err());
        assert!(!display_poll_timed_out(
            Duration::from_millis(999),
            Duration::from_secs(1)
        ));
        assert!(display_poll_timed_out(
            Duration::from_secs(1),
            Duration::from_secs(1)
        ));
        let primary = combine_display_mode_result(Err("readiness failed".into()), Ok(()))
            .unwrap_err()
            .to_string();
        assert_eq!(primary, "readiness failed");
        let combined =
            combine_display_mode_result(Err("capture failed".into()), Err("cancel failed".into()))
                .unwrap_err()
                .to_string();
        assert_eq!(
            combined,
            "capture failed; display rollback failed: cancel failed"
        );
    }

    #[test]
    fn png_dimensions_read_ihdr() {
        let mut png = vec![0u8; 24];
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&320u32.to_be_bytes());
        png[20..24].copy_from_slice(&240u32.to_be_bytes());
        assert_eq!(png_dimensions(&png).unwrap(), (320, 240));
    }
    use std::cell::RefCell;
    use std::fs;

    struct ScriptedDeployRemote {
        events: RefCell<Vec<String>>,
        fail_command_containing: Option<&'static str>,
        fail_upload: bool,
        remote_bytes: u64,
    }

    impl ScriptedDeployRemote {
        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    impl DeployRemote for ScriptedDeployRemote {
        fn exec(&self, command: &str) -> Result<ExecOutput> {
            self.events.borrow_mut().push(command.to_string());
            if self
                .fail_command_containing
                .is_some_and(|needle| command.contains(needle))
            {
                return Ok(ExecOutput {
                    rc: 9,
                    stdout: "scripted failure".to_string(),
                    stderr: String::new(),
                });
            }
            Ok(ExecOutput {
                rc: 0,
                stdout: if command.contains("wc -c") {
                    format!("{} remote\n", self.remote_bytes)
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }

        fn put(&self, local: &Path, remote: &str) -> Result<()> {
            self.events
                .borrow_mut()
                .push(format!("put {} {remote}", local.display()));
            if self.fail_upload {
                return Err("scripted upload failure".into());
            }
            Ok(())
        }
    }

    fn scripted_deploy_remote(remote_bytes: u64) -> ScriptedDeployRemote {
        ScriptedDeployRemote {
            events: RefCell::new(Vec::new()),
            fail_command_containing: None,
            fail_upload: false,
            remote_bytes,
        }
    }

    fn runtime_manifest_for(gui_sha256: &str) -> String {
        format!(
            "format=mister-magik-platform-v3\nplatform_release=platform-v0.1\nplatform_release_number=1\nplatform_bundle_id={hash}\nqualification_candidate_id={hash}\nlatch_protocol_version=4\nlatch_capability_mask=0x01ff\nmain_path=/media/fat/MiSTer_MagiKDev\ngui_path=/media/fat/mister-magik-dev/mister-magik-fb\nmanager_path=/media/fat/mister-magik-dev/mister-magik-manager\nscanout_module_path=/media/fat/mister-magik-dev/mister_magik_scanout_slots.ko\nscanout_metadata_path=/media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt\nlatch_rbf_path=/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf\nlatch_metadata_path=/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt\nmain_sha256={hash}\ngui_sha256={gui_sha256}\nmanager_sha256={hash}\nscanout_module_sha256={hash}\nscanout_metadata_sha256={hash}\nlatch_rbf_sha256={hash}\nlatch_metadata_sha256={hash}\nplatform_contract_sha256={hash}\nmain_revision={revision}\nmagik_revision={revision}\nmenu_revision={revision}\n",
            hash = "a".repeat(64),
            revision = "b".repeat(40),
        )
    }

    fn local_main_manifest_for(main_sha256: &str, gui_sha256: &str) -> String {
        let text = runtime_manifest_for(gui_sha256).replace(
            &format!("main_sha256={}", "a".repeat(64)),
            &format!("main_sha256={main_sha256}"),
        );
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let (key, value) = line.split_once('=').unwrap();
            values.insert(key.to_owned(), value.to_owned());
        }
        values.insert(
            "qualification_candidate_id".into(),
            local_main_candidate_id(&values),
        );
        RUNTIME_MANIFEST_FIELDS
            .iter()
            .map(|field| format!("{field}={}\n", values[*field]))
            .collect()
    }

    #[test]
    fn parses_sd_list_probe_options() {
        let args = vec![
            "/_Arcade".to_string(),
            "--protocol".to_string(),
            "v2".to_string(),
            "--show-hidden".to_string(),
            "--repeat".to_string(),
            "5".to_string(),
            "--json".to_string(),
        ];
        assert_eq!(
            parse_sd_list_options(&args).unwrap(),
            SdListOptions {
                path: "/_Arcade".to_string(),
                protocol: SdListProtocol::V2,
                show_hidden: true,
                repeat: 5,
                json: true,
            }
        );
    }

    #[test]
    fn rejects_invalid_sd_list_probe_options() {
        assert!(parse_sd_list_options(&[]).is_err());
        assert!(
            parse_sd_list_options(&["/".to_string(), "--repeat".to_string(), "0".to_string(),])
                .is_err()
        );
        assert!(
            parse_sd_list_options(&["/".to_string(), "--protocol".to_string(), "v3".to_string(),])
                .is_err()
        );
    }

    #[test]
    fn formats_compact_agent_magik_action_and_status_summaries() {
        let action = json!({
            "terminal_reason": "acknowledged",
            "after_generation": 8311,
            "main_status": {
                "launcher_state": "LauncherActive",
                "launcher_pid": 12711,
                "main_generation": 8311,
                "last_crash_reason": "large historical detail that must not leak",
            }
        });
        assert_eq!(
            format_agent_magik_summary("restart-launcher", 254, &action),
            "agent magik action=restart-launcher outcome=acknowledged elapsed_ms=254 state=LauncherActive pid=12711 generation=8311"
        );
        assert!(
            !format_agent_magik_summary("restart-launcher", 254, &action)
                .contains("historical detail")
        );

        let status = json!({
            "files": {"main_status": {"launcher_state": "LauncherSuspended"}}
        });
        assert_eq!(
            format_agent_magik_summary("status", 12, &status),
            "agent magik action=status outcome=ok elapsed_ms=12 state=LauncherSuspended pid=0 generation=0"
        );
    }

    #[test]
    fn framebuffer_capture_lz4_decode_is_exact_and_bounded_by_metadata() {
        let payload = lz4_flex::compress_prepend_size(b"pixels");
        assert_eq!(
            decompress_framebuffer_capture_lz4(&payload, 6).expect("decode capture"),
            b"pixels"
        );

        let err = decompress_framebuffer_capture_lz4(&payload, 5)
            .expect_err("mismatched prefix should fail before decode");
        assert!(err.to_string().contains("size prefix mismatch"));
    }

    fn status_fixture() -> Value {
        json!({
            "boot": {
                "ini_keys": {
                    "MiSTer": {
                        "main": {"value": "MiSTer_MagiK"},
                        "direct_video": {"value": "0"}
                    },
                    "arcade": {
                        "direct_video": {"value": "1", "line": 20}
                    },
                    "arcade_vertical": {
                        "direct_video": {"value": "0", "line": 24},
                        "video_mode": {"value": "8"}
                    },
                    "Menu": {
                        "direct_video": {"value": "0"},
                        "menu_pal": {"value": "0"},
                        "forced_scandoubler": {"value": "0"},
                        "video_mode": {"value": "8"}
                    }
                }
            },
            "processes": {
                "MiSTer": [],
                "MiSTer_MagiK": [{"pid": 10}],
                "mister-magik-fb": [{"pid": 11}]
            },
            "display": {
                "active_vt": "tty2",
                "fb0_visual": {"class": "slint_like"}
            },
            "runtime": {
                "main_status": {
                    "visible_owner": "fb0",
                    "launcher_state": "LauncherActive",
                    "launcher_ready_phase": "ready",
                    "launcher_ready_attempt": 1,
                    "launcher_ready_remaining_ms": 0,
                    "launcher_ready_last_failure": "none"
                },
                "slint_status": {}
            },
            "owners": {
                "by_device": {
                    "/dev/fb0": [{"process": "mister-magik-fb", "pid": 11, "fd": 5}]
                }
            }
        })
    }

    fn raw_frame_with<F>(f: F) -> Vec<u8>
    where
        F: FnMut(usize, usize) -> (u8, u8, u8),
    {
        raw_frame_with_geometry(default_fb_geometry(), f)
    }

    fn default_fb_geometry() -> FbGeometry {
        FbGeometry {
            width: DEFAULT_FB_W,
            height: DEFAULT_FB_H,
            stride: DEFAULT_FB_W * DEFAULT_FB_BPP / 8,
            bpp: DEFAULT_FB_BPP,
        }
    }

    fn raw_frame_with_geometry<F>(geometry: FbGeometry, mut f: F) -> Vec<u8>
    where
        F: FnMut(usize, usize) -> (u8, u8, u8),
    {
        let mut raw = vec![0; geometry.bytes().unwrap()];
        for y in 0..geometry.height {
            for x in 0..geometry.width {
                let (r, g, b) = f(x, y);
                let i = y * geometry.stride + x * 4;
                raw[i] = b;
                raw[i + 1] = g;
                raw[i + 2] = r;
                raw[i + 3] = 0xff;
            }
        }
        raw
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!("mister-tool-test-{name}-{}", unix_secs()));
        path
    }

    #[test]
    fn parses_relevant_ini_keys_with_sections_and_line_numbers() {
        let ini = r#"
; ignored
direct_video=1
[MiSTer]
direct_video=0
fb_terminal=1
fb_size=0
main=MiSTer_MagiK
[Menu]
video_mode=8
[arcade_vertical]
video_mode=14
"#;
        let parsed = parse_ini_keys(ini.to_string());
        assert_eq!(parsed["global"]["direct_video"]["value"], "1");
        assert_eq!(parsed["MiSTer"]["main"]["value"], "MiSTer_MagiK");
        assert_eq!(parsed["MiSTer"]["main"]["line"], 8);
        assert_eq!(parsed["Menu"]["video_mode"]["value"], "8");
        assert_eq!(parsed["arcade_vertical"]["video_mode"]["value"], "14");
        assert!(parsed["MiSTer"].get("unknown").is_none());
    }

    #[test]
    fn ini_parser_ignores_malformed_sections_and_comments() {
        let parsed = parse_ini_keys(
            "[MiSTer]\nmain=MiSTer_MagiK ; boot fork\n[broken\nvideo_mode=4\n# comment\n[Menu] ; inline note\nvideo_mode=8\n"
                .to_string(),
        );

        assert_eq!(
            parsed["MiSTer"]["main"]["value"],
            "MiSTer_MagiK ; boot fork"
        );
        assert_eq!(parsed["MiSTer"]["video_mode"]["value"], "4");
        assert_eq!(parsed["Menu"]["video_mode"]["value"], "8");
    }

    #[test]
    fn magik_boot_edit_sets_launcher_safe_video_without_touching_arcade_vertical() {
        let ini = "[MiSTer]\r\n; keep original core output for external scaler\r\ndirect_video=1\r\nmain=mister-magik-fb ; old handoff\r\n\r\n[arcade_vertical]\r\ndirect_video=0\r\nvideo_mode=14\r\nvscale_mode=1\r\n\r\n[Menu]\r\ndirect_video=0\r\nvideo_mode=4 ; menu probe\r\n";

        let edited = edit_mister_ini(ini, IniEdit::MagikBoot);

        assert!(edited.contains("direct_video=2\r\nmain=MiSTer_MagiK ; old handoff"));
        assert!(
            edited
                .contains("[arcade_vertical]\r\ndirect_video=0\r\nvideo_mode=14\r\nvscale_mode=1")
        );
        assert!(edited.contains("[Menu]\r\ndirect_video=0\r\nvideo_mode=8 ; menu probe"));
        assert!(edited.contains("; keep original core output for external scaler"));
    }

    #[test]
    fn select_main_preserves_other_settings_and_deduplicates_active_main() {
        let ini = "[MiSTer]\nmain=MiSTer\nfoo=keep\nmain=MiSTer_MagiK\n\n[Menu]\nvideo_mode=6\n";
        let edited = edit_mister_ini(ini, IniEdit::SelectMain("MiSTer_MagiKDev".into()));
        assert_eq!(edited.matches("main=MiSTer_MagiKDev").count(), 1);
        assert_eq!(
            edited
                .lines()
                .filter(|line| line.starts_with("main="))
                .count(),
            1
        );
        assert!(edited.contains("foo=keep"));
        assert!(edited.contains("[Menu]\nvideo_mode=6"));
    }

    #[test]
    fn alpha_host_restore_preserves_the_original_main_semantics() {
        let dev = "[MiSTer]\nmain=MiSTer_MagiK ; keep note\nfoo=keep\n";
        let restored = edit_mister_ini(dev, IniEdit::RestoreMain(Some("MiSTer_MagiKDev".into())));
        assert!(restored.contains("main=MiSTer_MagiKDev ; keep note"));
        assert!(restored.contains("foo=keep"));

        let absent = edit_mister_ini(
            "[MiSTer]\nmain=MiSTer_MagiK\nfoo=keep\n",
            IniEdit::RestoreMain(None),
        );
        let document = mister_magik_ini::Document::parse(absent.as_bytes()).unwrap();
        assert_eq!(document.effective_value("MiSTer", "main"), None);
        assert!(absent.contains("foo=keep"));
    }

    #[test]
    fn alpha_host_restore_accepts_only_supported_main_modes() {
        for value in [
            None,
            Some("MiSTer"),
            Some("MiSTer_MagiK"),
            Some("MiSTer_MagiKDev"),
        ] {
            assert!(require_alpha_host_main(value).is_ok());
        }
        assert!(require_alpha_host_main(Some("custom/unsafe")).is_err());
    }

    #[test]
    fn magik_boot_edit_defaults_to_auto_crt_with_hdmi_fallback() {
        let ini = "[MiSTer]\ndirect_video=1\n";
        let edited = edit_mister_ini(ini, IniEdit::MagikBoot);

        assert!(edited.contains(
            "[MiSTer]\ndirect_video=2\nforced_scandoubler=0\nmenu_pal=0\nmain=MiSTer_MagiK"
        ));
        assert!(edited.contains("[Menu]\nvideo_mode=8"));
    }

    #[test]
    fn menu_output_profiles_change_only_the_three_owned_menu_values() {
        let ini = "[MiSTer]\nmain=MiSTer_MagiKDev\ndirect_video=0\nmenu_pal=keep\nforced_scandoubler=keep\n\n[Menu]\nvideo_mode=6\ndirect_video=9 ; route note\nmenu_pal=9 ; region note\nforced_scandoubler=9 ; scan note\n\n[arcade]\ndirect_video=1\n";
        for (profile, direct_video, menu_pal, forced_scandoubler) in [
            (MenuOutputProfile::Hdmi, "0", "0", "0"),
            (MenuOutputProfile::Auto, "2", "0", "0"),
            (MenuOutputProfile::Crt240p60, "1", "0", "0"),
            (MenuOutputProfile::Crt288p50, "1", "1", "0"),
            (MenuOutputProfile::Crt480p60, "1", "0", "1"),
            (MenuOutputProfile::Crt576p50, "1", "1", "1"),
        ] {
            let edited = edit_mister_ini(ini, IniEdit::MenuOutput(profile));
            let expected = ini
                .replace(
                    "direct_video=9 ; route note",
                    &format!("direct_video={direct_video} ; route note"),
                )
                .replace(
                    "menu_pal=9 ; region note",
                    &format!("menu_pal={menu_pal} ; region note"),
                )
                .replace(
                    "forced_scandoubler=9 ; scan note",
                    &format!("forced_scandoubler={forced_scandoubler} ; scan note"),
                );
            assert_eq!(edited, expected);
        }
    }

    #[test]
    fn hdmi_menu_profiles_set_the_selected_mode_only_in_menu() {
        let ini = "[MiSTer]\nmain=MiSTer_MagiKDev\ndirect_video=7\nvideo_mode=4\n\n[Menu]\nvideo_mode=6 ; mode note\ndirect_video=9 ; route note\nmenu_pal=9 ; region note\nforced_scandoubler=9 ; scan note\n\n[arcade]\ndirect_video=1\nvideo_mode=5\n";
        for (name, mode) in [
            ("1280x720p60", "0"),
            ("1024x768p60", "1"),
            ("720x480p60", "2"),
            ("720x576p50", "3"),
            ("1280x1024p60", "4"),
            ("800x600p60", "5"),
            ("640x480p60", "6"),
            ("1280x720p50", "7"),
            ("1920x1080p60", "8"),
            ("1920x1080p50", "9"),
            ("1366x768p60", "10"),
            ("1024x600p60", "11"),
            ("1920x1440p60", "12"),
            ("2048x1536p60", "13"),
        ] {
            let profile = MenuOutputProfile::parse(name).expect("supported HDMI profile");
            assert_eq!(profile, MenuOutputProfile::HdmiMode(mode));
            let edited = edit_mister_ini(ini, IniEdit::MenuOutput(profile));
            let expected = ini
                .replace(
                    "video_mode=6 ; mode note",
                    &format!("video_mode={mode} ; mode note"),
                )
                .replace("direct_video=9 ; route note", "direct_video=0 ; route note")
                .replace("menu_pal=9 ; region note", "menu_pal=0 ; region note")
                .replace(
                    "forced_scandoubler=9 ; scan note",
                    "forced_scandoubler=0 ; scan note",
                );
            assert_eq!(edited, expected, "HDMI profile {name}");
        }
    }

    #[test]
    fn mode_14_returns_the_required_error_before_ini_editing() {
        let error = parse_ini_edit_args(&["menu".into(), "2560x1440p60".into()])
            .expect_err("mode 14 must be rejected");
        assert_eq!(error.to_string(), "Mister does not support 1440p");
    }

    #[test]
    fn hdmi_menu_profile_preserves_crlf_and_creates_only_menu() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiKDev\r\n";
        let edited = edit_mister_ini(ini, IniEdit::MenuOutput(MenuOutputProfile::HdmiMode("10")));
        assert_eq!(
            edited,
            "[MiSTer]\r\nmain=MiSTer_MagiKDev\r\n\r\n[Menu]\r\ndirect_video=0\r\nmenu_pal=0\r\nforced_scandoubler=0\r\nvideo_mode=10\r\n"
        );
    }

    #[test]
    fn menu_output_profile_preserves_crlf_and_appends_only_missing_owned_keys() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiKDev\r\ndirect_video=0\r\n\r\n[Menu]\r\nvideo_mode=6 ; untouched\r\n";
        let edited = edit_mister_ini(ini, IniEdit::MenuOutput(MenuOutputProfile::Crt576p50));
        assert_eq!(
            edited,
            "[MiSTer]\r\nmain=MiSTer_MagiKDev\r\ndirect_video=0\r\n\r\n[Menu]\r\nvideo_mode=6 ; untouched\r\ndirect_video=1\r\nmenu_pal=1\r\nforced_scandoubler=1\r\n"
        );
    }

    #[test]
    fn menu_output_profile_creates_only_a_missing_menu_section() {
        let ini = "[MiSTer]\nmain=MiSTer_MagiKDev\n";
        let edited = edit_mister_ini(ini, IniEdit::MenuOutput(MenuOutputProfile::Crt240p60));
        assert_eq!(
            edited,
            "[MiSTer]\nmain=MiSTer_MagiKDev\n\n[Menu]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n"
        );
    }

    #[test]
    fn zaparoo_boot_edit_selects_zaparoo_fork_and_launcher_safe_video() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiK ; current launcher\r\ndirect_video=1\r\n\r\n[Menu]\r\nvideo_mode=6\r\n";

        let edited = edit_mister_ini(ini, IniEdit::ZaparooBoot);

        assert!(edited.contains("main=zaparoo/MiSTer_Zaparoo ; current launcher\r\n"));
        assert!(edited.contains("direct_video=0\r\n"));
        assert!(edited.contains("[Menu]\r\nvideo_mode=8\r\n"));
    }

    #[test]
    fn menu_output_profile_preserves_main_and_menu_video_mode() {
        let ini = "[MiSTer]\nmain=MiSTer_MagiK\nforced_scandoubler=0\nmenu_pal=0\ndirect_video=1\n\n[Menu]\nvideo_mode=8\n";
        let crt = edit_mister_ini(ini, IniEdit::MenuOutput(MenuOutputProfile::Crt576p50));
        assert!(
            crt.contains("[Menu]\nvideo_mode=8\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1")
        );
        assert!(crt.contains("main=MiSTer_MagiK"));
        assert!(crt.contains("video_mode=8"));
    }

    #[test]
    fn stock_boot_restore_comments_only_magik_main_with_crlf_and_inline_comment() {
        let ini = "[MiSTer]\r\nmain=MiSTer_MagiK ; keep note\r\ndirect_video=1\r\n\r\n[Menu]\r\nvideo_mode=8\r\n";

        let edited = edit_mister_ini(ini, IniEdit::StockBoot);

        assert!(
            edited.contains(";main=MiSTer_MagiK ; keep note ; MiSTer MagiK stock boot restore\r\n")
        );
        assert!(edited.contains("direct_video=1\r\n"));
        assert!(edited.contains("[Menu]\r\nvideo_mode=8\r\n"));
    }

    #[test]
    fn development_menu_restore_removes_benchmark_overrides_only() {
        let ini = "[MiSTer]\nmain=MiSTer\nforced_scandoubler=0\n\n[Menu]\nvideo_mode=0\ndirect_video=0\nmenu_pal=0\n";

        let edited = edit_mister_ini(ini, IniEdit::RestoreDevelopmentMenu);

        assert!(edited.contains("main=MiSTer_MagiKDev"));
        assert!(edited.contains(";video_mode=0 ; MiSTer MagiK benchmark override removed"));
        assert!(edited.contains(";direct_video=0 ; MiSTer MagiK benchmark override removed"));
        assert!(edited.contains("menu_pal=0"));
        assert!(edited.contains("forced_scandoubler=0"));
    }

    #[test]
    fn stock_boot_restore_leaves_missing_or_unrelated_main_alone() {
        let missing = "[Menu]\nvideo_mode=8\n";
        assert_eq!(edit_mister_ini(missing, IniEdit::StockBoot), missing);

        let unrelated = "[MiSTer]\nmain=Some_Other_Menu\n";
        assert_eq!(edit_mister_ini(unrelated, IniEdit::StockBoot), unrelated);
    }

    #[test]
    fn stock_boot_restore_is_idempotent_for_commented_main() {
        let ini = "[MiSTer]\n;main=MiSTer_MagiK ; already disabled\n";
        assert_eq!(edit_mister_ini(ini, IniEdit::StockBoot), ini);
    }

    #[test]
    fn stock_boot_restore_comments_legacy_direct_slint_handoff() {
        let ini = "[MiSTer]\nmain=mister-magik-fb\n";
        let edited = edit_mister_ini(ini, IniEdit::StockBoot);

        assert!(edited.contains(";main=mister-magik-fb ; MiSTer MagiK stock boot restore"));
    }

    #[test]
    fn crt_trial_requires_main_to_report_a_standard_crt_mode() {
        for mode in ["crt-240p60", "crt-288p50", "crt-480p60", "crt-576p50"] {
            let reply = format!("ok SettingsV1 schema=1 output={mode}\n");
            assert_eq!(
                parse_crt_runtime_settings_reply(&reply).unwrap(),
                format!("schema=1&output={mode}")
            );
        }
        assert!(parse_crt_runtime_settings_reply("ok SettingsV1 schema=1 output=hdmi").is_err());
    }

    #[test]
    fn crt_trial_status_requires_successful_shared_latch_publication() {
        let valid = "crt_trial_status_v2 schema=2 ok=1 mode=crt-288p50 duration_ms=30001 frames=1500 flips=1500 reason=none\n";
        assert_eq!(parse_crt_trial_status(valid).unwrap(), valid.trim());
        let diagnostic = "crt_trial_status_v3 schema=3 ok=1 mode=crt-576p50 duration_ms=30001 frames=1513 flips=1513 posts=1513 drops=0 final_pending=0 final_active_matches=1 unsafe_active_writes=0 pending_writes=0 alternation_misses=0 cadence_misses=0 max_interval_us=20500 max_settle_us=18000 max_render_us=1000 max_copy_us=500 max_status_us=200 post_status_retry_frames=1 max_post_status_reads=2 last_buffer=1 last_sequence=1513 reason=none\n";
        assert_eq!(
            parse_crt_trial_status(diagnostic).unwrap(),
            diagnostic.trim()
        );
        let wire_diagnostic = "crt_trial_status_v4 schema=4 ok=1 mode=crt-576p50 duration_ms=30001 frames=1513 flips=1513 posts=1513 drops=0 final_pending=0 final_active_matches=1 unsafe_active_writes=0 pending_writes=0 alternation_misses=0 cadence_misses=0 max_interval_us=20500 max_settle_us=18000 max_render_us=1000 max_copy_us=500 max_status_us=200 post_status_retry_frames=0 max_post_status_reads=1 post_status_transport_retry_frames=1 max_post_status_wire_attempts=2 last_buffer=1 last_sequence=1513 reason=none\n";
        assert_eq!(
            parse_crt_trial_status(wire_diagnostic).unwrap(),
            wire_diagnostic.trim()
        );
        let mixed_versions = format!(
            "older output\n{diagnostic}unrelated trailer\n{wire_diagnostic}final trailer\n"
        );
        assert_eq!(
            parse_crt_trial_status(&mixed_versions).unwrap(),
            wire_diagnostic.trim()
        );
        let failure = parse_crt_trial_status(
            "crt_trial_status_v2 schema=2 ok=0 mode=crt-240p60 duration_ms=12 frames=0 flips=0 reason=no-latch-flips"
        )
        .unwrap_err()
        .to_string();
        assert!(failure.contains("reason=no-latch-flips"));
        let appended = format!("runtime log without trailing newline {valid}");
        assert_eq!(parse_crt_trial_status(&appended).unwrap(), valid.trim());
        assert!(
            parse_crt_trial_status(
                "crt_trial_status_v3 schema=3 ok=1 mode=crt-576p50 duration_ms=30001 frames=1513 flips=1513 reason=none"
            )
            .is_err()
        );
        assert!(parse_crt_trial_status("untyped success").is_err());
    }

    #[test]
    fn crt_trial_command_is_bounded_and_never_changes_output_routes() {
        let command = crt_trial_run_command("schema=1&output=crt-480p60", None);
        assert!(command.contains("trap cleanup EXIT HUP INT TERM"));
        assert!(command.contains("mister_magik_resume"));
        assert!(command.contains("schema=1&output=crt-480p60"));
        assert!(command.contains(" ui crt_trial 30 "));
        assert!(!command.contains("settings_set"));
        assert!(!command.contains("launcher.env"));
    }

    #[test]
    fn crt_screensaver_trial_is_bounded_self_restoring_and_uses_the_product_launcher() {
        let command = crt_screensaver_trial_run_command("schema=1&output=crt-576p50", 30);

        assert!(command.contains("MISTER_SCREENSAVER_START_ACTIVE=1"));
        assert!(command.contains(" ui launcher 30 "));
        assert!(command.contains("mister_magik_resume"));
        assert!(command.contains("rm -f /tmp/mister-magik/realtime-frame-analytics"));
        assert!(command.contains("crt-screensaver-status.json"));
        assert!(!command.contains("MiSTer.ini"));
    }

    #[test]
    fn crt_screensaver_matrix_trial_fits_inside_the_headless_transaction_window() {
        let command = crt_screensaver_trial_run_command("schema=1&output=crt-240p60", 8);

        assert!(command.contains(" ui launcher 8 "));
        assert!(!command.contains(" ui launcher 30 "));
    }

    #[test]
    fn crt_screensaver_matrix_contains_exactly_the_four_standard_crt_modes() {
        assert_eq!(
            crt_screensaver_matrix_modes()
                .map(|mode| mode.id)
                .collect::<Vec<_>>(),
            ["crt-240p60", "crt-288p50", "crt-480p60", "crt-576p50"]
        );
    }

    #[test]
    fn geometry_trials_are_mode_bounded_and_change_only_one_axis() {
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-288p50", [67, 706, 14, 297]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-288p50", [67, 706, 32, 286]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-288p50", [66, 706, 14, 297]).is_err()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [40, 679, 40, 615]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [40, 679, 41, 615]).is_err()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [32, 607, 40, 614]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [32, 607, 40, 606]).is_err()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [40, 680, 40, 615]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [0, 511, 40, 615]).is_ok()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-576p50", [0, 491, 40, 615]).is_err()
        );
        assert!(
            validate_crt_geometry_trial("schema=1&output=crt-480p60", [45, 684, 31, 510]).is_err()
        );

        let command = crt_trial_run_command("schema=1&output=crt-576p50", Some([40, 679, 40, 615]));
        assert!(command.contains("MISTER_MAGIK_CRT_TRIAL=1"));
        assert!(command.contains("MISTER_FB_DIAGNOSTIC_RECT=45,684,40,615"));
        assert!(command.contains("MISTER_CRT_TRIAL_CONTENT_BOUNDS=40,679"));
        assert!(!command.contains("launcher.env"));

        let command = crt_trial_run_command("schema=1&output=crt-288p50", Some([67, 706, 32, 286]));
        assert!(command.contains("MISTER_FB_DIAGNOSTIC_RECT=67,706,32,286"));
        assert!(!command.contains("MISTER_CRT_TRIAL_CONTENT_BOUNDS"));
    }

    #[test]
    fn geometry_trial_usb_capture_uses_the_camera_jpeg_contract() {
        assert_eq!(
            crt_geometry_capture_path(Path::new("/tmp"), 1234),
            Path::new("/tmp/mister-magik-crt-geometry-1234.jpg")
        );
    }

    #[test]
    fn launcher_restart_args_collect_env_and_timeout() {
        let args = vec![
            "--env".to_string(),
            "MISTER_LAUNCHER_START_SCREEN=arcade".to_string(),
            "--env".to_string(),
            "MISTER_PREVIEW_SCROLL_TRACE=/tmp/trace.tsv".to_string(),
            "--timeout".to_string(),
            "30".to_string(),
        ];

        let options = parse_launcher_restart_args(&args).unwrap();

        assert_eq!(options.timeout_secs, 30);
        assert_eq!(options.remote_env, DEFAULT_LAUNCHER_ENV_REMOTE);
        assert_eq!(
            options.env_vars,
            vec![
                (
                    "MISTER_LAUNCHER_START_SCREEN".to_string(),
                    "arcade".to_string()
                ),
                (
                    "MISTER_PREVIEW_SCROLL_TRACE".to_string(),
                    "/tmp/trace.tsv".to_string()
                )
            ]
        );
    }

    #[test]
    fn launcher_restart_args_reject_bad_env_and_clear_conflict() {
        assert!(
            parse_launcher_restart_args(&["--env".to_string(), "BAD-NAME=value".to_string()])
                .is_err()
        );
        assert!(
            parse_launcher_restart_args(&[
                "--clear-env".to_string(),
                "--env".to_string(),
                "MISTER_CATALOG_REFRESH=off".to_string()
            ])
            .is_err()
        );
        assert!(
            parse_launcher_restart_args(&[
                "--clear-env".to_string(),
                "--remote-env".to_string(),
                "relative/launcher.env".to_string()
            ])
            .is_err()
        );
    }

    #[test]
    fn launcher_env_text_shell_quotes_values() {
        let text = launcher_env_text(&[
            ("MISTER_CATALOG_REFRESH".to_string(), "off".to_string()),
            ("MISTER_LABEL".to_string(), "kid's test".to_string()),
        ]);

        assert!(text.contains("export MISTER_CATALOG_REFRESH='off'\n"));
        assert!(text.contains("export MISTER_LABEL='kid'\"'\"'s test'\n"));
    }

    #[test]
    fn one_shot_launcher_env_removes_itself_after_being_sourced() {
        let text = one_shot_launcher_env_text(
            &[("MISTER_CATALOG_REFRESH".into(), "off".into())],
            DEVELOPMENT_LAUNCHER_ENV_REMOTE,
        );

        assert!(text.contains("export MISTER_CATALOG_REFRESH='off'"));
        assert!(text.ends_with("rm -f '/media/fat/mister-magik-dev/launcher.env'\n"));
    }

    #[test]
    fn library_db_query_preserves_statement_for_remote_read_only_validation() {
        let args = vec![
            "--path".to_string(),
            "/tmp/library.sqlite3".to_string(),
            "-- comment\n/* more */ WITH recent AS (SELECT 'delete from games')".to_string(),
            "SELECT * FROM recent".to_string(),
        ];

        let (path, queries) = parse_library_db_queries(&args).expect("read-only query");

        assert_eq!(path, "/tmp/library.sqlite3");
        assert!(queries[0].contains("WITH recent"));
    }

    #[test]
    fn library_db_query_accepts_pragma_for_remote_read_only_validation() {
        let (_, queries) =
            parse_library_db_queries(&["PRAGMA table_info(launch_plans)".to_string()])
                .expect("pragma should reach SQLite read-only validation");
        assert_eq!(queries, ["PRAGMA table_info(launch_plans)"]);
    }

    #[test]
    fn library_db_query_parses_repeated_query_batch() {
        let (path, queries) = parse_library_db_queries(&[
            "--path".to_string(),
            "/tmp/library.sqlite3".to_string(),
            "--query".to_string(),
            "SELECT count(*) FROM game_rows".to_string(),
            "--query".to_string(),
            "PRAGMA table_info(launch_plans)".to_string(),
        ])
        .expect("query batch");

        assert_eq!(path, "/tmp/library.sqlite3");
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0], "SELECT count(*) FROM game_rows");
        assert_eq!(queries[1], "PRAGMA table_info(launch_plans)");
    }

    #[test]
    fn launcher_remote_env_parent_requires_absolute_path() {
        assert_eq!(
            remote_parent_dir("/media/fat/mister-magik/launcher.env").unwrap(),
            "/media/fat/mister-magik"
        );
        assert_eq!(remote_parent_dir("/launcher.env").unwrap(), "/");
        assert!(remote_parent_dir("relative/launcher.env").is_err());
    }

    #[test]
    fn launcher_ready_requires_main_and_new_slint_status() {
        let main = json!({
            "launcher_state": "LauncherActive",
            "launcher_pid": 42
        });
        let slint = json!({
            "scene": "launcher",
            "pid": 43,
            "frames": 2,
            "screen": "arcade"
        });

        assert!(launcher_ready_status(125, Some(&main), Some(&slint)).is_none());

        let slint = json!({
            "scene": "launcher",
            "pid": 42,
            "frames": 2,
            "screen": "arcade"
        });
        let ready = launcher_ready_status(125, Some(&main), Some(&slint)).unwrap();

        assert_eq!(ready.launcher_pid, 42);
        assert_eq!(ready.slint_pid, 42);
        assert_eq!(ready.frames, 2);
        assert_eq!(ready.screen, "arcade");
        assert!(launcher_ready_status(125, Some(&main), None).is_none());
        assert!(
            launcher_ready_status(
                125,
                Some(&main),
                Some(&json!({"scene": "launcher", "frames": 0}))
            )
            .is_none()
        );
    }

    #[test]
    fn reboot_remote_command_supervised_uses_magik_command() {
        let cmd = reboot_remote_command(RebootMode::Supervised);

        assert!(cmd.contains("mister_magik_reboot"));
        assert!(cmd.contains("/dev/MiSTer_cmd"));
        assert!(cmd.contains("MiSTer_MagiK"));
        assert!(!cmd.contains("/sbin/reboot"));
    }

    #[test]
    fn reboot_remote_command_raw_uses_linux_reboot() {
        let cmd = reboot_remote_command(RebootMode::Raw);

        assert!(cmd.contains("/sbin/reboot"));
        assert!(!cmd.contains("mister_magik_reboot"));
    }

    #[test]
    fn delivery_reboot_uses_the_running_main_capability() {
        assert_eq!(
            delivery_reboot_mode("MiSTer_MagiKDev", Some("LauncherActive")),
            RebootMode::Supervised
        );
        assert_eq!(
            delivery_reboot_mode("MiSTer_MagiK", Some("LauncherActive")),
            RebootMode::Supervised
        );
        assert_eq!(
            delivery_reboot_mode("MiSTer_MagiKDev", Some("Unconfigured")),
            RebootMode::Raw
        );
        assert_eq!(
            delivery_reboot_mode("MiSTer_MagiKDev", None),
            RebootMode::Raw
        );
        assert_eq!(
            delivery_reboot_mode("MiSTer", Some("LauncherActive")),
            RebootMode::Raw
        );
        assert_eq!(delivery_reboot_mode("", None), RebootMode::Raw);
    }

    #[test]
    fn reboot_remote_command_direct_reset_uses_explicit_unsafe_fifo_command() {
        let cmd = reboot_remote_command(RebootMode::DirectReset);

        assert!(cmd.contains("mister_magik_direct_reset"));
        assert!(cmd.contains("/dev/MiSTer_cmd"));
        assert!(!cmd.contains("/sbin/reboot"));
    }

    #[test]
    fn reboot_remote_command_direct_reset_no_sync_uses_distinct_fifo_command() {
        let cmd = reboot_remote_command(RebootMode::DirectResetNoSync);

        assert!(cmd.contains("mister_magik_direct_reset_no_sync"));
        assert!(cmd.contains("/dev/MiSTer_cmd"));
        assert!(!cmd.contains("/sbin/reboot"));
    }

    #[test]
    fn reboot_defaults_to_supervised_and_mode_flag_is_removed_before_timeout_parse() {
        let mut args = vec!["--raw".to_string(), "180".to_string()];

        assert_eq!(take_reboot_mode_flag(&mut args).unwrap(), RebootMode::Raw);
        assert_eq!(args, vec!["180"]);
        assert_eq!(
            take_reboot_mode_flag(&mut args).unwrap(),
            RebootMode::Supervised
        );
    }

    #[test]
    fn reboot_mode_flags_conflict() {
        let mut args = vec!["--raw".to_string(), "--supervised".to_string()];

        assert!(take_reboot_mode_flag(&mut args).is_err());
        assert!(
            reboot_mode_from_args(&["--direct-reset".to_string(), "--raw".to_string()]).is_err()
        );
    }

    #[test]
    fn reboot_mode_from_args_accepts_direct_reset_modes() {
        assert_eq!(
            reboot_mode_from_args(&["--direct-reset".to_string()]).unwrap(),
            RebootMode::DirectReset
        );
        assert_eq!(
            reboot_mode_from_args(&["--direct-reset-no-sync".to_string()]).unwrap(),
            RebootMode::DirectResetNoSync
        );
    }

    #[test]
    fn reboot_recovery_status_accepts_scripted_readiness_transitions_once() {
        let mut measurement = RebootRecoveryMeasurement::default();
        apply_reboot_status(&mut measurement, None, None, 10);
        assert_eq!(measurement, RebootRecoveryMeasurement::default());

        apply_reboot_status(
            &mut measurement,
            Some(&json!({"launcher_state": "LauncherStarting"})),
            None,
            25,
        );
        apply_reboot_status(
            &mut measurement,
            Some(&json!({"launcher_state": "LauncherActive"})),
            Some(&json!({"frames": 3})),
            40,
        );

        assert_eq!(measurement.main_status_ms, Some(25));
        assert_eq!(measurement.slint_status_ms, Some(40));
        assert_eq!(measurement.launcher_state, "LauncherStarting");
        assert_eq!(measurement.slint_frames, "3");
    }

    #[test]
    fn stock_inittab_mutator_removes_old_magik_entries() {
        let input = "::sysinit:/bin/mount -a\r\n::sysinit:/media/fat/MiSTer_MagiK &\r\n::sysinit:/media/fat/mister-magik/boot.sh &\r\n";

        let edited = ensure_stock_inittab_text(input);

        assert!(edited.contains("::sysinit:/bin/mount -a\r\n"));
        assert!(edited.contains("::sysinit:/media/fat/MiSTer &\r\n"));
        assert!(!edited.contains("MiSTer_MagiK"));
        assert!(!edited.contains("mister-magik/boot.sh"));
    }

    #[test]
    fn stock_inittab_mutator_deduplicates_stock_entry() {
        let input =
            "::sysinit:/media/fat/MiSTer &\n::sysinit:/media/fat/MiSTer &\n::respawn:/sbin/getty\n";

        let edited = ensure_stock_inittab_text(input);

        assert_eq!(edited.matches("::sysinit:/media/fat/MiSTer &").count(), 1);
        assert!(edited.contains("::respawn:/sbin/getty\n"));
    }

    #[test]
    fn status_prefers_launcher_process_over_helper_processes() {
        let status = json!({
            "processes": {
                "mister-magik-fb": [
                    {"pid": 1661, "cmdline": "/media/fat/mister-magik/mister-magik-fb library-refresh"},
                    {"pid": 1528, "cmdline": "/media/fat/mister-magik/mister-magik-fb ui launcher 0"}
                ]
            }
        });

        assert_eq!(
            primary_process(&status, "mister-magik-fb").and_then(|process| process["pid"].as_u64()),
            Some(1528)
        );
    }

    #[test]
    fn arcade_video_edit_sets_normal_direct_and_vertical_1080p() {
        let ini = "[MiSTer]\ndirect_video=0\nmain=MiSTer_MagiK\n\n[arcade_vertical]\ndirect_video=0\nvideo_mode=14\nvscale_mode=1\n";

        let edited = edit_mister_ini(ini, IniEdit::ArcadeVideo);

        assert!(edited.contains("[MiSTer]\ndirect_video=0\nmain=MiSTer_MagiK"));
        assert!(edited.contains("[arcade]\ndirect_video=1"));
        assert!(edited.contains("[arcade_vertical]\ndirect_video=0\nvideo_mode=8\nvscale_mode=1"));
        assert!(edited.find("[arcade]\n").unwrap() < edited.find("[arcade_vertical]\n").unwrap());
    }

    #[test]
    fn ini_edit_accepts_only_menu_profiles_and_stock_boot() {
        for profile in [
            "hdmi",
            "auto",
            "crt-240p60",
            "crt-288p50",
            "crt-480p60",
            "crt-576p50",
            "1280x720p60",
            "1024x768p60",
            "720x480p60",
            "720x576p50",
            "1280x1024p60",
            "800x600p60",
            "640x480p60",
            "1280x720p50",
            "1920x1080p60",
            "1920x1080p50",
            "1366x768p60",
            "1024x600p60",
            "1920x1440p60",
            "2048x1536p60",
        ] {
            assert!(parse_ini_edit_args(&["menu".into(), profile.into()]).is_ok());
        }
        assert_eq!(
            parse_ini_edit_args(&["stock-boot".into()]).unwrap(),
            IniEdit::StockBoot
        );
        assert_eq!(
            parse_ini_edit_args(&["restore-development-menu".into()]).unwrap(),
            IniEdit::RestoreDevelopmentMenu
        );
        for retired in [
            vec!["magik-boot".into()],
            vec!["magik-boot-hdmi".into()],
            vec!["magik-boot-crt-240p60".into()],
            vec!["crt".into(), "1".into(), "0".into(), "0".into()],
            vec!["menu-mode".into(), "8".into()],
            vec!["menu-auto".into()],
            vec!["zaparoo-boot".into()],
            vec!["arcade-video".into()],
            vec!["comment-main".into()],
        ] {
            assert!(parse_ini_edit_args(&retired).is_err());
        }
    }

    #[test]
    fn filters_inittab_lines_by_needles() {
        let lines = lines_containing(
            "::sysinit:/media/fat/MiSTer &\n::respawn:/sbin/getty tty1\nboot.sh mister-magik\n"
                .to_string(),
            &["MiSTer", "mister-magik"],
        );
        assert_eq!(
            lines,
            vec![
                "::sysinit:/media/fat/MiSTer &".to_string(),
                "boot.sh mister-magik".to_string()
            ]
        );
    }

    #[test]
    fn parses_input_devices_into_names_handlers_and_ids() {
        let devices = parse_input_devices(
            r#"I: Bus=0003 Vendor=2563 Product=0575 Version=0111
N: Name="Retro-bit Controller"
H: Handlers=js0 event4

I: Bus=0003 Vendor=0000 Product=0000 Version=0004
N: Name="MiSTer virtual input"
H: Handlers=sysrq kbd event7
"#
            .to_string(),
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["name"], "Retro-bit Controller");
        assert_eq!(devices[0]["handlers"], json!(["js0", "event4"]));
        assert_eq!(
            devices[1]["id"],
            "Bus=0003 Vendor=0000 Product=0000 Version=0004"
        );
    }

    #[test]
    fn parses_input_devices_without_trailing_blank_line() {
        let devices = parse_input_devices(
            r#"I: Bus=0003 Vendor=045e Product=028e Version=0114
N: Name="Xbox 360 Controller"
H: Handlers=event3 js0"#
                .to_string(),
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["name"], "Xbox 360 Controller");
        assert_eq!(devices[0]["handlers"], json!(["event3", "js0"]));
    }

    #[test]
    fn classifies_black_slint_and_static_like_framebuffers() {
        let geometry = default_fb_geometry();
        let black = vec![0; geometry.bytes().unwrap()];
        assert_eq!(classify_fb(&black, &geometry)["class"], "mostly_black");

        let slint = raw_frame_with(|x, _| {
            if x < DEFAULT_FB_W / 2 {
                (0x06, 0xd6, 0xa0)
            } else {
                (0xe8, 0xe0, 0xf0)
            }
        });
        assert_eq!(classify_fb(&slint, &geometry)["class"], "slint_like");

        let static_like = raw_frame_with(|x, y| {
            if (x / 16 + y / 16) % 2 == 0 {
                (0xff, 0xff, 0xff)
            } else {
                (0x10, 0x10, 0x10)
            }
        });
        assert_eq!(classify_fb(&static_like, &geometry)["class"], "static_like");
    }

    #[test]
    fn parses_virtual_size() {
        assert_eq!(parse_virtual_size("960,540"), Some((960, 540)));
        assert_eq!(parse_virtual_size(" 1920,1080\n"), Some((1920, 1080)));
        assert_eq!(parse_virtual_size("bad"), None);
        assert_eq!(parse_virtual_size("960x540"), None);
        assert_eq!(parse_virtual_size("960,"), None);
    }

    #[test]
    fn framebuffer_geometry_bytes_detects_overflow() {
        let geometry = FbGeometry {
            width: 1,
            height: usize::MAX,
            stride: 2,
            bpp: 16,
        };

        assert!(
            geometry
                .bytes()
                .unwrap_err()
                .to_string()
                .contains("overflow")
        );
    }

    #[test]
    fn classifies_strided_960x540_framebuffer() {
        let geometry = FbGeometry {
            width: 960,
            height: 540,
            stride: 4096,
            bpp: 32,
        };
        let raw = raw_frame_with_geometry(geometry, |x, _| {
            if x < 480 {
                (0x06, 0xd6, 0xa0)
            } else {
                (0xe8, 0xe0, 0xf0)
            }
        });
        assert_eq!(raw.len(), 4096 * 540);
        assert_eq!(classify_fb(&raw, &geometry)["width"], 960);
        assert_eq!(classify_fb(&raw, &geometry)["height"], 540);
        assert_eq!(classify_fb(&raw, &geometry)["stride"], 4096);
        assert_eq!(classify_fb(&raw, &geometry)["class"], "slint_like");
    }

    #[test]
    fn doctor_reports_ok_for_nominal_launcher_state() {
        let findings = doctor_findings(&status_fixture());
        assert_eq!(
            findings,
            vec![(
                "ok".to_string(),
                "No obvious launcher/display problems found".to_string()
            )]
        );
    }

    #[test]
    fn doctor_reports_actionable_failures() {
        let mut status = status_fixture();
        status["boot"]["ini_keys"]["MiSTer"]["main"]["value"] = json!("mister-magik-fb");
        status["boot"]["ini_keys"]["arcade"]["direct_video"]["value"] = json!("0");
        status["boot"]["ini_keys"]["Menu"]["direct_video"]["value"] = json!("9");
        status["boot"]["ini_keys"]["Menu"]["menu_pal"]["value"] = json!("9");
        status["boot"]["ini_keys"]["Menu"]["video_mode"]["value"] = json!("6");
        status["processes"]["mister-magik-fb"] = json!([]);
        status["display"]["active_vt"] = json!("tty1");
        status["display"]["fb0_visual"]["class"] = json!("mostly_black");
        status["runtime"]["main_status"]["visible_owner"] = json!("menu_bg");
        status["owners"]["by_device"]["/dev/fb0"] = json!([]);

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(&"[MiSTer] main is not MiSTer_MagiK"));
        assert!(texts.contains(&"[Menu] direct_video is not HDMI (0), CRT (1), or automatic (2)"));
        assert!(texts.contains(&"[Menu] menu_pal is not 0 or 1"));
        assert!(texts.contains(
            &"[arcade] direct_video is not 1; normal arcade games will use scaler output"
        ));
        assert!(texts.contains(&"mister-magik-fb is not running"));
        assert!(texts.contains(&"/dev/fb0 samples as mostly_black"));
        assert!(texts.contains(&"Main reports visible_owner=menu_bg rather than fb0"));
        assert!(texts.contains(&"/dev/fb0 is not owned by mister-magik-fb"));
    }

    #[test]
    fn doctor_reports_incomplete_launcher_readiness_and_stock_menu_fallback() {
        let mut status = status_fixture();
        status["runtime"]["main_status"]["launcher_state"] = json!("LauncherStarting");
        status["runtime"]["main_status"]["launcher_ready_phase"] = json!("awaiting");
        status["runtime"]["main_status"]["launcher_ready_attempt"] = json!(2);
        status["runtime"]["main_status"]["launcher_ready_remaining_ms"] = json!(3210);
        status["runtime"]["main_status"]["launcher_ready_last_failure"] = json!("ready-timeout");

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(
            &"launcher readiness is still awaiting (attempt 2, 3210ms remaining; last failure ready-timeout)"
        ));

        status["runtime"]["main_status"]["launcher_state"] = json!("Unconfigured");
        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(
            texts.contains(&"launcher readiness fallback restored stock Menu after ready-timeout")
        );
    }

    #[test]
    fn doctor_reports_multiple_magik_framebuffer_owners() {
        let mut status = status_fixture();
        status["processes"]["mister-magik-fb"] = json!([
            {"pid": 11, "cmdline": "/media/fat/mister-magik/mister-magik-fb"},
            {"pid": 12, "cmdline": "/media/fat/mister-magik/mister-magik-fb ui launcher 0"}
        ]);
        status["owners"]["by_device"]["/dev/fb0"] = json!([
            {"process": "mister-magik-fb", "pid": 11, "fd": 5},
            {"process": "mister-magik-fb", "pid": 12, "fd": 5}
        ]);

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(&"multiple mister-magik-fb processes own /dev/fb0: 11,12"));
    }

    #[test]
    fn doctor_reports_arcade_vertical_section_order_regression() {
        let mut status = status_fixture();
        status["boot"]["ini_keys"]["arcade"]["direct_video"]["line"] = json!(30);
        status["boot"]["ini_keys"]["arcade_vertical"]["direct_video"]["line"] = json!(20);

        let findings = doctor_findings(&status);
        let texts: Vec<_> = findings.iter().map(|(_, text)| text.as_str()).collect();
        assert!(texts.contains(
            &"[arcade] appears after [arcade_vertical]; vertical arcade settings will be overwritten"
        ));
    }

    #[test]
    fn display_read_requires_unsafe_spi_when_main_is_running() {
        let status = status_fixture();
        assert!(display_read_needs_unsafe_spi(&status));

        let mut no_main = status;
        no_main["processes"]["MiSTer_MagiK"] = json!([]);
        no_main["processes"]["MiSTer"] = json!([]);
        assert!(!display_read_needs_unsafe_spi(&no_main));
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(sh("/tmp/simple"), "'/tmp/simple'");
        assert_eq!(sh("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn delivery_process_lock_is_nonblocking_and_released_on_drop() {
        let device = format!("test-{}", std::process::id());
        let first = DeliveryProcessLock::acquire(&device).unwrap();
        assert!(matches!(
            DeliveryProcessLock::acquire(&device),
            Err(DeviceFailure::Busy(_))
        ));
        drop(first);
        assert!(DeliveryProcessLock::acquire(&device).is_ok());
    }

    #[test]
    fn manager_fetch_requires_one_exact_manifest_identity() {
        let expected = "a".repeat(64);
        assert!(manifest_has_manager(
            &format!("format=manifest\nmanager_sha256={expected}\n"),
            &expected
        ));
        assert!(!manifest_has_manager(
            &format!("manager_sha256={expected}\nmanager_sha256={expected}\n"),
            &expected
        ));
        assert!(!manifest_has_manager(
            &format!("manager_sha256={}\n", "b".repeat(64)),
            &expected
        ));
    }

    #[test]
    fn local_main_reload_requires_an_active_dev_main_that_advertises_support() {
        let supported = json!({
            "local_main_reload_supported": true,
            "launcher_state": "LauncherActive",
            "executable_path": LOCAL_MAIN_REMOTE,
            "pid": 41,
            "main_generation": 9001,
        });
        assert_eq!(
            local_main_activation(Some(&supported)),
            LocalMainActivation::SupervisedReload {
                pid: 41,
                generation: 9001,
            }
        );
        for status in [
            Value::Null,
            json!({
                "launcher_state": "LauncherActive",
                "executable_path": LOCAL_MAIN_REMOTE,
                "pid": 41,
                "main_generation": 9001,
            }),
            json!({
                "local_main_reload_supported": true,
                "launcher_state": "LauncherSuspended",
                "executable_path": LOCAL_MAIN_REMOTE,
                "pid": 41,
                "main_generation": 9001,
            }),
            json!({
                "local_main_reload_supported": true,
                "launcher_state": "LauncherActive",
                "executable_path": "/media/fat/MiSTer_MagiK",
                "pid": 41,
                "main_generation": 9001,
            }),
        ] {
            assert_eq!(
                local_main_activation((status != Value::Null).then_some(&status)),
                LocalMainActivation::LinuxReboot
            );
        }
    }

    #[test]
    fn local_main_transaction_swaps_main_before_manifest_and_restores_the_pair() {
        let snapshot = local_main_snapshot_script();
        assert!(snapshot.contains("local-main.delivery-state"));
        assert!(snapshot.contains("snapshot"));

        let swap = local_main_swap_script(&"a".repeat(64), &"b".repeat(64));
        let main_swap = swap.find("MiSTer_MagiKDev.upload").unwrap();
        let manifest_swap = swap.find("platform-v3.manifest.upload").unwrap();
        assert!(main_swap < manifest_swap);
        assert!(swap.contains("chmod 755"));
        assert!(swap.find("activating").unwrap() < main_swap);

        let rollback = local_main_rollback_script();
        assert!(rollback.contains("MiSTer_MagiKDev.delivery-rollback"));
        assert!(rollback.contains("platform-v3.manifest.delivery-rollback"));
        assert!(rollback.contains("cp -p"));
        assert!(rollback.contains("rolled-back"));
        assert!(
            rollback.find("MiSTer_MagiKDev.delivery-rollback").unwrap()
                < rollback
                    .find("platform-v3.manifest.delivery-rollback")
                    .unwrap()
        );

        let reconcile = local_main_reconcile_script();
        assert!(reconcile.contains("validated"));
        assert!(reconcile.contains("local-main.delivery-state"));
        assert!(!local_main_reconcile_requires_recovery(
            "local-main-reconcile=none\n"
        ));
        assert!(!local_main_reconcile_requires_recovery(
            "local-main-reconcile=snapshot\n"
        ));
        assert!(local_main_reconcile_requires_recovery(
            "local-main-reconcile=activating\n"
        ));
        assert!(local_main_reconcile_requires_recovery(
            "local-main-reconcile=rolled-back\n"
        ));
        assert!(local_main_cleanup_script().contains("validated"));
        assert!(local_main_rollback_cleanup_script().contains("rolled-back"));
    }

    #[test]
    fn local_main_bundle_requires_exact_main_and_preserved_gui_identities() {
        let local = temp_path("local-main-bin");
        let manifest = temp_path("local-main-manifest");
        fs::write(&local, b"local main").unwrap();
        let main_sha256 = file_sha256(local.clone()).unwrap();
        let gui_sha256 = "c".repeat(64);
        let text = local_main_manifest_for(&main_sha256, &gui_sha256);
        fs::write(&manifest, text).unwrap();
        assert!(
            validate_local_main_bundle_identity(&local, &manifest, &main_sha256, &gui_sha256)
                .is_ok()
        );
        assert!(
            validate_local_main_bundle_identity(&local, &manifest, &"d".repeat(64), &gui_sha256)
                .is_err()
        );
        assert!(
            validate_local_main_bundle_identity(&local, &manifest, &main_sha256, &"e".repeat(64))
                .is_err()
        );
        let installed =
            parse_local_main_manifest_text(&local_main_manifest_for(&"a".repeat(64), &gui_sha256))
                .unwrap();
        let candidate = parse_local_main_manifest(&manifest).unwrap();
        assert!(validate_local_main_overlay_preserves_installed(&installed, &candidate).is_ok());
        let mut changed_rbf = candidate;
        changed_rbf.insert("latch_rbf_sha256".into(), "f".repeat(64));
        assert!(validate_local_main_overlay_preserves_installed(&installed, &changed_rbf).is_err());
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[derive(Default)]
    struct ScriptedCoherentDelivery {
        events: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
    }

    impl ScriptedCoherentDelivery {
        fn step(&mut self, name: &'static str) -> std::result::Result<(), DeviceFailure> {
            self.events.push(name);
            if self.fail_at == Some(name) || (name == "rollback" && self.rollback_fails) {
                Err(DeviceFailure::OperationFailed(name.into()))
            } else {
                Ok(())
            }
        }
    }

    impl CoherentDeliveryActions for ScriptedCoherentDelivery {
        fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("snapshot")
        }

        fn deploy(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("deploy")
        }

        fn activate(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("activate")
        }

        fn reboot(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("reboot")
        }

        fn smoke(&mut self) -> std::result::Result<String, DeviceFailure> {
            self.step("smoke").map(|()| "healthy".into())
        }

        fn commit(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("commit")
        }

        fn rollback(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("rollback")
        }

        fn health(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("health")
        }
    }

    #[test]
    fn coherent_runtime_commits_without_reboot() {
        let mut actions = ScriptedCoherentDelivery::default();
        assert_eq!(
            run_coherent_delivery(&mut actions, false).unwrap(),
            "healthy"
        );
        assert_eq!(
            actions.events,
            ["snapshot", "deploy", "activate", "smoke", "commit"]
        );
    }

    #[test]
    fn coherent_platform_reboots_and_commits_after_smoke() {
        let mut actions = ScriptedCoherentDelivery::default();
        run_coherent_delivery(&mut actions, true).unwrap();
        assert_eq!(
            actions.events,
            [
                "snapshot", "deploy", "activate", "reboot", "smoke", "commit"
            ]
        );
    }

    #[test]
    fn coherent_delivery_rolls_back_and_verifies_health_after_failure() {
        let mut runtime = ScriptedCoherentDelivery {
            fail_at: Some("smoke"),
            ..ScriptedCoherentDelivery::default()
        };
        assert!(run_coherent_delivery(&mut runtime, false).is_err());
        assert_eq!(
            runtime.events,
            [
                "snapshot", "deploy", "activate", "smoke", "rollback", "health"
            ]
        );

        let mut platform = ScriptedCoherentDelivery {
            fail_at: Some("smoke"),
            ..ScriptedCoherentDelivery::default()
        };
        assert!(run_coherent_delivery(&mut platform, true).is_err());
        assert_eq!(
            platform.events,
            [
                "snapshot", "deploy", "activate", "reboot", "smoke", "rollback", "reboot", "health"
            ]
        );
    }

    #[test]
    fn coherent_delivery_reports_failed_rollback_as_recovery_required() {
        let mut actions = ScriptedCoherentDelivery {
            fail_at: Some("deploy"),
            rollback_fails: true,
            ..ScriptedCoherentDelivery::default()
        };
        assert!(matches!(
            run_coherent_delivery(&mut actions, false),
            Err(DeviceFailure::RecoveryRequired(_))
        ));
    }

    #[test]
    fn coherent_delivery_does_not_rollback_after_commit_cleanup_starts() {
        let mut actions = ScriptedCoherentDelivery {
            fail_at: Some("commit"),
            ..ScriptedCoherentDelivery::default()
        };
        assert!(matches!(
            run_coherent_delivery(&mut actions, false),
            Err(DeviceFailure::RecoveryRequired(_))
        ));
        assert_eq!(
            actions.events,
            ["snapshot", "deploy", "activate", "smoke", "commit"]
        );
    }

    #[test]
    fn runtime_rollback_attempts_resume_when_restore_fails() {
        let events = RefCell::new(Vec::new());
        let result = restore_and_resume(
            || {
                events.borrow_mut().push("restore");
                Err(DeviceFailure::OperationFailed("restore".into()))
            },
            || {
                events.borrow_mut().push("resume");
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(*events.borrow(), ["restore", "resume"]);
    }

    #[test]
    fn runtime_deploy_requires_the_canonical_binary_and_manifest_bundle() {
        let local = temp_path("deploy-bin");
        let manifest = temp_path("deploy-manifest");
        fs::write(&local, b"abc").unwrap();
        let expected = file_sha256(local.clone()).unwrap();
        fs::write(&manifest, runtime_manifest_for(&expected)).unwrap();

        assert!(
            MagikDeployTransaction::validate_bundle(
                &local,
                "/media/fat/mister-magik/mister-magik-fb",
                &manifest,
                "/media/fat/mister-magik/platform-v3.manifest",
                &expected,
            )
            .is_err()
        );
        let tx = MagikDeployTransaction::validate_bundle(
            &local,
            "/media/fat/mister-magik-dev/mister-magik-fb",
            &manifest,
            "/media/fat/mister-magik-dev/platform-v3.manifest",
            &expected,
        )
        .unwrap();

        assert_eq!(tx.remote_dir, "/media/fat/mister-magik-dev");
        assert_eq!(
            tx.upload,
            "/media/fat/mister-magik-dev/mister-magik-fb.upload"
        );
        assert_eq!(tx.lock, "/media/fat/mister-magik-dev/deploy.lock");
        assert_eq!(tx.local_bytes, 3);
        assert_eq!(
            tx.chmod_size_verify_command(),
            "chmod +x '/media/fat/mister-magik-dev/mister-magik-fb' && wc -c '/media/fat/mister-magik-dev/mister-magik-fb'"
        );
        assert!(
            MagikDeployTransaction::validate_bundle(
                &local,
                "/media/fat/mister-magik-dev/mister-magik-fb",
                &manifest,
                "/media/fat/mister-magik-dev/platform-v3.manifest",
                &"c".repeat(64),
            )
            .is_err()
        );
        for invalid in [
            runtime_manifest_for(&expected)
                .replace("format=mister-magik-platform-v3", "format=wrong"),
            runtime_manifest_for(&expected).replace("gui_sha256=", "wrong_gui_sha256="),
            format!(
                "{}gui_sha256={}\n",
                runtime_manifest_for(&expected),
                expected
            ),
        ] {
            fs::write(&manifest, invalid).unwrap();
            assert!(
                MagikDeployTransaction::validate_bundle(
                    &local,
                    "/media/fat/mister-magik-dev/mister-magik-fb",
                    &manifest,
                    "/media/fat/mister-magik-dev/platform-v3.manifest",
                    &expected,
                )
                .is_err()
            );
        }
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[test]
    fn runtime_bundle_uploads_both_files_and_activates_manifest_last() {
        let local = temp_path("deploy-bundle-bin");
        let manifest = temp_path("deploy-bundle-manifest");
        fs::write(&local, b"abc").unwrap();
        let expected = file_sha256(local.clone()).unwrap();
        fs::write(&manifest, runtime_manifest_for(&expected)).unwrap();
        let tx = MagikDeployTransaction::validate_bundle(
            &local,
            "/media/fat/mister-magik-dev/mister-magik-fb",
            &manifest,
            "/media/fat/mister-magik-dev/platform-v3.manifest",
            &expected,
        )
        .unwrap();
        let remote = scripted_deploy_remote(3);

        tx.run_with(&remote, 0, Instant::now()).unwrap();
        let events = remote.events();
        assert!(events[2].ends_with("mister-magik-fb.upload"));
        assert!(events[3].ends_with("platform-v3.manifest.upload"));
        assert!(events[4].contains("sha256sum"));
        assert!(
            events[5].find("mister-magik-fb.upload").unwrap()
                < events[5].find("platform-v3.manifest.upload").unwrap()
        );
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[test]
    fn deploy_transaction_cleans_and_resumes_after_upload_failure() {
        let local = temp_path("deploy-scripted-upload-failure");
        let manifest = temp_path("deploy-scripted-upload-failure-manifest");
        fs::write(&local, b"abc").unwrap();
        let expected = file_sha256(local.clone()).unwrap();
        fs::write(&manifest, runtime_manifest_for(&expected)).unwrap();
        let tx = MagikDeployTransaction::validate_bundle(
            &local,
            "/media/fat/mister-magik-dev/mister-magik-fb",
            &manifest,
            "/media/fat/mister-magik-dev/platform-v3.manifest",
            &expected,
        )
        .unwrap();
        let mut remote = scripted_deploy_remote(3);
        remote.fail_upload = true;

        assert!(tx.run_with(&remote, 0, Instant::now()).is_err());
        let events = remote.events();

        assert!(events[1].contains("mister_magik_suspend"));
        assert!(events[2].starts_with("put "));
        assert!(events[3].starts_with("rm -f "));
        assert!(events[4].contains("mister_magik_resume"));
        assert_eq!(events.len(), 5);
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[test]
    fn runtime_bundle_rejects_corrupt_uploads_before_activation() {
        let local = temp_path("deploy-corrupt-upload");
        let manifest = temp_path("deploy-corrupt-upload-manifest");
        fs::write(&local, b"abc").unwrap();
        let expected = file_sha256(local.clone()).unwrap();
        fs::write(&manifest, runtime_manifest_for(&expected)).unwrap();
        let tx = MagikDeployTransaction::validate_bundle(
            &local,
            "/media/fat/mister-magik-dev/mister-magik-fb",
            &manifest,
            "/media/fat/mister-magik-dev/platform-v3.manifest",
            &expected,
        )
        .unwrap();
        let mut remote = scripted_deploy_remote(3);
        remote.fail_command_containing = Some("sha256sum");

        assert!(tx.run_with(&remote, 0, Instant::now()).is_err());
        let events = remote.events();
        assert!(!events.iter().any(|event| event.contains("; mv ")));
        assert!(
            events
                .last()
                .is_some_and(|event| event.contains("mister_magik_resume"))
        );
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[test]
    fn deploy_transaction_cleans_partial_prepare_failure() {
        let local = temp_path("deploy-scripted-prepare-failure");
        let manifest = temp_path("deploy-scripted-prepare-failure-manifest");
        fs::write(&local, b"abc").unwrap();
        let expected = file_sha256(local.clone()).unwrap();
        fs::write(&manifest, runtime_manifest_for(&expected)).unwrap();
        let tx = MagikDeployTransaction::validate_bundle(
            &local,
            "/media/fat/mister-magik-dev/mister-magik-fb",
            &manifest,
            "/media/fat/mister-magik-dev/platform-v3.manifest",
            &expected,
        )
        .unwrap();
        let mut remote = scripted_deploy_remote(3);
        remote.fail_command_containing = Some("mkdir -p");

        assert!(tx.run_with(&remote, 0, Instant::now()).is_err());
        let events = remote.events();

        assert_eq!(events.len(), 2);
        assert!(events[0].contains("mkdir -p"));
        assert!(events[1].starts_with("rm -f "));
        assert!(!events.iter().any(|event| event.contains("suspend")));
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
    }

    #[test]
    fn parse_profile_count_skips_option_values() {
        assert_eq!(
            parse_profile_count(&["--timeout".to_string(), "30".to_string()], 1),
            1
        );
        assert_eq!(
            parse_profile_count(
                &[
                    "--timeout".to_string(),
                    "30".to_string(),
                    "4".to_string(),
                    "--bytes".to_string(),
                    "1024".to_string(),
                ],
                1,
            ),
            4
        );
    }

    #[test]
    fn deploy_size_parsing_reads_busybox_wc_prefix() {
        assert_eq!(
            parse_wc_byte_count("12345 /media/fat/mister-magik/mister-magik-fb\n"),
            Some(12345)
        );
        assert_eq!(parse_wc_byte_count("not-a-size path\n"), None);
    }

    #[test]
    fn option_value_reads_next_arg() {
        let args = vec![
            "--settle".to_string(),
            "12".to_string(),
            "--keep-enabled".to_string(),
            "--item".to_string(),
            "first".to_string(),
            "--item".to_string(),
            "second".to_string(),
        ];
        assert_eq!(option_value(&args, "--settle"), Some("12".to_string()));
        assert_eq!(option_value(&args, "--missing"), None);
        assert_eq!(
            option_values(&args, "--item"),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn option_values_do_not_treat_following_flags_as_values() {
        let args = vec![
            "--software-list".to_string(),
            "nes.xml".to_string(),
            "--software-list".to_string(),
            "--software-dir".to_string(),
            "lists".to_string(),
            "--offset".to_string(),
            "-1".to_string(),
            "--out".to_string(),
            "--dry-run".to_string(),
            "--out".to_string(),
            "build/mame.sqlite3".to_string(),
        ];

        assert_eq!(
            option_value(&args, "--software-list"),
            Some("nes.xml".to_string())
        );
        assert_eq!(
            option_values(&args, "--software-list"),
            vec!["nes.xml".to_string()]
        );
        assert_eq!(option_value(&args, "--offset"), Some("-1".to_string()));
        assert_eq!(
            option_value(&args, "--out"),
            Some("build/mame.sqlite3".to_string())
        );
        assert_eq!(option_value(&args, "--missing"), None);
    }

    #[test]
    fn parses_mame_1942_metadata() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let parent = machines
            .iter()
            .find(|machine| machine.setname == "1942")
            .unwrap();
        let clone = machines
            .iter()
            .find(|machine| machine.setname == "1942a")
            .unwrap();

        assert_eq!(parent.parent_setname, None);
        assert_eq!(parent.title, "1942 (Revision B)");
        assert_eq!(parent.year.as_deref(), Some("1984"));
        assert_eq!(parent.manufacturer.as_deref(), Some("Capcom"));
        assert_eq!(parent.rotate, Some(270));
        assert_eq!(parent.display_width, Some(256));
        assert_eq!(parent.display_height, Some(224));
        assert_eq!(parent.players, Some(2));
        assert_eq!(parent.coins, Some(2));
        assert_eq!(parent.control_type.as_deref(), Some("joy"));
        assert_eq!(parent.control_ways.as_deref(), Some("8"));
        assert_eq!(parent.buttons, Some(2));
        assert_eq!(parent.driver_status.as_deref(), Some("good"));
        assert_eq!(parent.source_version, "0.288 (mame0288)");
        assert_eq!(clone.parent_setname.as_deref(), Some("1942"));
    }

    #[test]
    fn writes_mame_metadata_sqlite() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let path = temp_path("mame.sqlite3");
        write_mame_metadata_db(&path, &machines, &[], &[]).unwrap();
        let conn = Connection::open(&path).unwrap();
        let row: (String, String, i64, i64, i64, String) = conn
            .query_row(
                "SELECT parent_setname, manufacturer, rotate, buttons, players, control_type
                 FROM mame_machines WHERE setname='1942a'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(
            row,
            (
                "1942".to_string(),
                "Capcom".to_string(),
                270,
                2,
                2,
                "joy".to_string()
            )
        );
    }

    #[test]
    fn loads_mame_machines_from_existing_sqlite() {
        let machines = parse_mame_listxml(MAME_1942_FIXTURE).unwrap();
        let path = temp_path("mame-machine-source.sqlite3");
        write_mame_metadata_db(&path, &machines, &[], &[]).unwrap();
        let loaded = load_mame_machines_from_db(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(loaded.iter().any(|machine| {
            machine.setname == "1942a"
                && machine.parent_setname.as_deref() == Some("1942")
                && machine.buttons == Some(2)
        }));
    }

    #[test]
    fn parses_mame_software_list_items_and_hashes() {
        let (items, hashes) = parse_mame_software_list_xml(
            r#"
            <softwarelist name="saturn" description="Saturn">
              <software name="nights" cloneof="nightsu">
                <description>Nights into Dreams (Europe)</description>
                <year>1996</year>
                <publisher>Sega</publisher>
                <part name="cdrom" interface="saturn_cdrom">
                  <diskarea name="cdrom">
                    <disk name="nights" sha1="ABCDEF0123456789ABCDEF0123456789ABCDEF01"/>
                  </diskarea>
                </part>
              </software>
              <software name="sonic">
                <description>Sonic the Hedgehog (USA)</description>
                <year>1991</year>
                <publisher>Sega</publisher>
                <part name="cart" interface="megadriv_cart">
                  <dataarea name="rom" size="524288">
                    <rom name="sonic.bin" size="524288" crc="F9394E97" sha1="0123456789ABCDEF0123456789ABCDEF01234567"/>
                  </dataarea>
                </part>
              </software>
            </softwarelist>
            "#,
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].list_name, "saturn");
        assert_eq!(items[0].software_name, "nights");
        assert_eq!(items[0].parent_name.as_deref(), Some("nightsu"));
        assert_eq!(items[0].region.as_deref(), Some("europe"));
        assert_eq!(hashes.len(), 2);
        assert_eq!(
            hashes[0].disk_sha1.as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(hashes[1].crc32.as_deref(), Some("f9394e97"));
        assert_eq!(
            hashes[1].sha1.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn profile_summary_reports_frame_count_and_percentiles() {
        let path = temp_path("profile.tsv");
        fs::write(
            &path,
            "frame\twall_us\trender_us\tcopy_us\n0\t10\t100\t7\n1\t20\t200\t9\n2\t30\t300\t11\n",
        )
        .unwrap();
        let text = profile_summary_text(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(text.contains("(3 frames)"));
        assert!(text.contains("wall_us"));
        assert!(text.contains("min=    10"));
        assert!(text.contains("p50=    20"));
        assert!(text.contains("render_us"));
        assert!(text.contains("copy_us"));
    }

    #[test]
    fn capture_buffer_paths_follow_screenshot_naming() {
        let desktop = Path::new("/Users/example/Desktop");
        assert_eq!(
            desktop_capture_path(desktop, "2026-07-20 at 14.32.08", 1),
            desktop.join("MiSTer Framebuffer 2026-07-20 at 14.32.08.png")
        );
        assert_eq!(
            desktop_capture_path(desktop, "2026-07-20 at 14.32.08", 2),
            desktop.join("MiSTer Framebuffer 2026-07-20 at 14.32.08 2.png")
        );
    }

    #[test]
    fn capture_buffer_rejects_arguments_before_device_work() {
        assert!(validate_capture_buffer_args(&[]).is_ok());
        assert_eq!(
            validate_capture_buffer_args(&["extra".to_string()])
                .unwrap_err()
                .to_string(),
            "usage: mister --capture-buffer"
        );
    }

    #[test]
    fn capture_buffer_requires_png_signature() {
        assert!(validate_png(b"\x89PNG\r\n\x1a\nfixture").is_ok());
        assert!(validate_png(b"not png").is_err());
        assert!(validate_png(&[]).is_err());
    }

    #[test]
    fn capture_buffer_writes_timestamped_temporary_files_without_overwriting() {
        let root = temp_path("capture-temporary");
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let first = write_temporary_capture_at(&root, 1_753_012_345_678, png).unwrap();
        let second = write_temporary_capture_at(&root, 1_753_012_345_678, png).unwrap();
        let captures = fs::canonicalize(root.join("mister-magik/captures")).unwrap();

        assert_eq!(first.parent(), Some(captures.as_path()));
        assert_eq!(
            first.file_name().unwrap(),
            "mister-magik-framebuffer-1753012345678.png"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "mister-magik-framebuffer-1753012345678-2.png"
        );
        assert_eq!(fs::read(&first).unwrap(), png);
        assert_eq!(fs::read(&second).unwrap(), png);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_buffer_markdown_link_contains_only_the_absolute_path() {
        let path = Path::new("/private/tmp/mister-magik/captures/framebuffer fixture.png");
        let output = capture_markdown_link(path);

        assert_eq!(
            output,
            "[MiSTer framebuffer](</private/tmp/mister-magik/captures/framebuffer fixture.png>)"
        );
        assert!(!output.contains("data"));
        assert!(!output.contains("iVBOR"));
    }

    #[test]
    fn delivery_smoke_capture_summary_excludes_image_payload() {
        let capture = PngCapture {
            result: json!({
                "schema": "mister-magik-framebuffer-capture-v2",
                "source": "fpga-latched-scanout-slots",
                "capture_source": {
                    "kind": "fpga-latched-scanout-slots",
                    "active_base": "0x30000000",
                    "active_sequence": 2,
                    "region_index": 0,
                    "region_name": "hidden-slot-1"
                },
                "authoritative_scanout": true,
                "width": 640,
                "height": 480,
                "stride": 1280,
                "bpp": 16,
                "png_bytes": 123_456,
                "content_nonzero_bytes": 100,
                "content_varied": true,
                "png_hex": "89504e470d0a1a0a-secret-image-data",
            }),
            png: b"\x89PNG\r\n\x1a\nsecret-image-data".to_vec(),
            elapsed_ms: 42,
        };

        let summary = delivery_smoke_capture_detail(&capture).unwrap();

        assert!(summary.contains("capture=verified"));
        assert!(summary.contains("width=640"));
        assert!(summary.contains("height=480"));
        assert!(summary.contains("png_bytes=123456"));
        assert!(!summary.contains("data"));
        assert!(!summary.contains("png_hex"));
        assert!(!summary.contains("89504e470d0a1a0a"));
        assert!(summary.len() < 256);
    }

    fn delivery_status(
        effective_view: &str,
        return_screen: &str,
        backend: &str,
        present_status: &str,
    ) -> Value {
        json!({
            "scene": "launcher",
            "screen": effective_view,
            "effective_view": effective_view,
            "return_screen": return_screen,
            "present_backend": backend,
            "present_status": present_status,
            "launch_state": "idle",
            "input_enabled": true,
            "compatibility_prompt_visible": effective_view == "compatibility"
        })
    }

    fn terminal_compatibility_evidence() -> Value {
        json!({
            "schema": "mister-magik-latch-failure-v2",
            "state": "runtime-fault",
            "stage": "latch-post",
            "reason": "latch-post-failed",
            "detail": "first failure",
            "latest_state": "runtime-fault",
            "latest_stage": "post-verification",
            "latest_reason": "posted-sequence-unverified",
            "latest_detail": "retry failure",
            "attempt_count": 1,
            "latest_result": "failure",
            "recovery_state": "compatibility-prompt"
        })
    }

    #[test]
    fn delivery_accepts_latch_or_terminal_evidenced_compatibility() {
        let mut latch = delivery_status(
            "screensaver",
            "screensaver-settings",
            "fpga-vblank-latch-hidden",
            "ok",
        );
        latch
            .as_object_mut()
            .unwrap()
            .remove("compatibility_prompt_visible");
        assert_eq!(
            validate_delivery_present_state(&latch, None).unwrap(),
            DeliveryPresentState::Latch
        );

        let mut compatibility = delivery_status(
            "compatibility",
            "settings",
            "compatibility-fb0",
            "compatibility",
        );
        compatibility["input_enabled"] = json!(false);
        assert_eq!(
            validate_delivery_present_state(
                &compatibility,
                Some(&terminal_compatibility_evidence())
            )
            .unwrap(),
            DeliveryPresentState::Compatibility
        );

        let continued =
            delivery_status("settings", "settings", "compatibility-fb0", "compatibility");
        let mut continued_evidence = terminal_compatibility_evidence();
        continued_evidence["recovery_state"] = json!("continued-compatibility");
        assert_eq!(
            validate_delivery_present_state(&continued, Some(&continued_evidence)).unwrap(),
            DeliveryPresentState::Compatibility
        );

        let mut structured = terminal_compatibility_evidence();
        structured["schema"] = json!("mister-magik-latch-failure-v3");
        structured["wire_diagnostics"] = json!({
            "attempt_count": 1,
            "decision": "rejected"
        });
        assert_eq!(
            validate_delivery_present_state(&compatibility, Some(&structured)).unwrap(),
            DeliveryPresentState::Compatibility
        );
    }

    #[test]
    fn delivery_rejects_split_view_state_and_nonterminal_recovery() {
        let mut split =
            delivery_status("screensaver", "settings", "fpga-vblank-latch-hidden", "ok");
        split["screen"] = json!("settings");
        assert!(
            validate_delivery_present_state(&split, None)
                .unwrap_err()
                .to_string()
                .contains("view mismatch")
        );

        let compatibility = delivery_status(
            "compatibility",
            "settings",
            "compatibility-fb0",
            "compatibility",
        );
        let mut evidence = terminal_compatibility_evidence();
        evidence["recovery_state"] = json!("automatic-retry");
        assert!(
            validate_delivery_present_state(&compatibility, Some(&evidence))
                .unwrap_err()
                .to_string()
                .contains("not terminal")
        );
    }

    #[test]
    fn delivery_rejects_active_launch_and_unexplained_fallback() {
        let mut launching =
            delivery_status("launching", "arcade", "fpga-vblank-latch-hidden", "ok");
        launching["launch_state"] = json!("launching");
        assert!(
            validate_delivery_present_state(&launching, None)
                .unwrap_err()
                .to_string()
                .contains("not interactive")
        );

        let compatibility = delivery_status(
            "compatibility",
            "settings",
            "compatibility-fb0",
            "compatibility",
        );
        assert!(
            validate_delivery_present_state(&compatibility, None)
                .unwrap_err()
                .to_string()
                .contains("missing latch failure evidence")
        );
    }

    #[test]
    fn capture_contract_rejects_stale_missing_metadata() {
        let stale = json!({
            "schema": "mister-magik-framebuffer-capture-v1",
            "source": "fb0"
        });
        assert!(
            validate_capture_contract(&stale)
                .unwrap_err()
                .to_string()
                .contains("unsupported schema")
        );
    }

    #[test]
    fn transitional_scanout_capture_errors_are_the_only_retryable_capture_failures() {
        assert!(is_transient_authoritative_capture_error(
            "device_operation_failed: authoritative scanout capture failed: latched framebuffer status is not active"
        ));
        assert!(is_transient_authoritative_capture_error(
            "authoritative scanout capture failed: no scanout slot matches active base 0x22001000"
        ));
        assert!(!is_transient_authoritative_capture_error(
            "authoritative scanout capture failed: active base is not a hidden slot"
        ));
        assert!(!is_transient_authoritative_capture_error(
            "agent framebuffer capture returned invalid PNG data"
        ));
    }

    #[test]
    fn capture_contract_accepts_diagnostic_producer_composition() {
        let result = json!({
            "schema": "mister-magik-framebuffer-capture-v2",
            "source": "producer-composition",
            "capture_source": {
                "kind": "producer-composition",
                "sequence": 17,
                "authoritative_error": "active base is not a hidden slot"
            },
            "authoritative_scanout": false,
            "content_nonzero_bytes": 100,
            "content_varied": true
        });

        validate_capture_contract(&result).unwrap();
        let capture = PngCapture {
            result,
            png: vec![],
            elapsed_ms: 0,
        };
        assert!(validate_visible_launcher_capture(&capture).is_err());
    }

    #[test]
    fn launcher_smoke_rejects_fallback_and_blank_authoritative_capture() {
        let mut result = json!({
            "schema": "mister-magik-framebuffer-capture-v2",
            "source": "fb0",
            "capture_source": {"kind": "fb0"},
            "authoritative_scanout": false,
            "width": 640,
            "height": 480,
            "stride": 1280,
            "bpp": 16,
            "content_nonzero_bytes": 100,
            "content_varied": true
        });
        let capture = PngCapture {
            result: result.clone(),
            png: vec![],
            elapsed_ms: 0,
        };
        assert!(validate_visible_launcher_capture(&capture).is_err());

        result["source"] = json!("fpga-latched-scanout-slots");
        result["capture_source"] = json!({
            "kind": "fpga-latched-scanout-slots",
            "active_base": "0x30000000",
            "active_sequence": 2,
            "region_index": 0,
            "region_name": "hidden-slot-1"
        });
        result["authoritative_scanout"] = json!(true);
        result["content_nonzero_bytes"] = json!(0);
        result["content_varied"] = json!(false);
        let capture = PngCapture {
            result,
            png: vec![],
            elapsed_ms: 0,
        };
        assert!(validate_visible_launcher_capture(&capture).is_err());
    }

    #[test]
    fn rgb565_stride_accepts_aligned_padding_but_rejects_invalid_rows() {
        assert!(valid_rgb565_stride(683, 1_376));
        assert!(valid_rgb565_stride(960, 1_920));
        assert!(!valid_rgb565_stride(683, 1_366 - 2));
        assert!(!valid_rgb565_stride(683, 1_367));
        assert!(!valid_rgb565_stride(0, 0));
    }

    #[test]
    fn capture_buffer_writes_real_collision_safe_desktop_files() {
        let root = temp_path("capture-desktop");
        let desktop = root.join("Desktop");
        fs::create_dir_all(&desktop).unwrap();
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let first = write_desktop_capture_at(&desktop, "2026-07-20 at 14.32.08", png).unwrap();
        let second = write_desktop_capture_at(&desktop, "2026-07-20 at 14.32.08", png).unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "MiSTer Framebuffer 2026-07-20 at 14.32.08.png"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "MiSTer Framebuffer 2026-07-20 at 14.32.08 2.png"
        );
        assert_eq!(fs::read(first).unwrap(), png);
        assert_eq!(fs::read(second).unwrap(), png);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_buffer_rejects_missing_desktop() {
        let desktop = temp_path("missing-desktop").join("Desktop");
        let error = write_desktop_capture_at(&desktop, "2026-07-20 at 14.32.08", b"png")
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("Desktop directory does not exist:"));
    }

    #[test]
    fn retired_platform_cli_actions_are_rejected_before_device_use() {
        for action in [
            "platform-deploy",
            "platform-deliver",
            "platform-rollback",
            "platform-commit",
        ] {
            let error = reject_retired_platform_command(action)
                .unwrap_err()
                .to_string();
            assert_eq!(error, RETIRED_PLATFORM_COMMAND_ERROR);
            assert!(!action_uses_device(action));
        }

        assert!(reject_retired_platform_command("platform-status").is_ok());
        assert!(action_uses_device("platform-status"));
    }

    #[test]
    fn host_usage_does_not_advertise_platform_deploy_entrypoints() {
        assert!(!CLI_USAGE.contains("platform-deploy"));
        assert!(!CLI_USAGE.contains("platform-deliver"));
        assert!(!CLI_USAGE.contains("platform-rollback"));
        assert!(!CLI_USAGE.contains("platform-commit"));
    }

    #[test]
    fn platform_deploy_validates_every_required_file_and_publishes_manifest_last() {
        let stage = temp_path("platform-stage");
        for (relative, _) in PLATFORM_DEPLOY_FILES {
            let path = stage.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, relative.as_bytes()).unwrap();
        }
        let transaction = PlatformDeployTransaction::validate(&stage).unwrap();
        let changed = transaction.files.iter().collect::<Vec<_>>();
        let script = transaction.activation_script(&changed);
        let gui = script
            .find("mister-magik-fb.upload' '/media/fat/mister-magik-dev/mister-magik-fb'")
            .unwrap();
        let manifest = script
            .find("platform-v3.manifest.upload' '/media/fat/mister-magik-dev/platform-v3.manifest'")
            .unwrap();
        assert!(manifest > gui);
        assert!(script.contains("trap rollback EXIT INT TERM"));
        assert!(script.contains("platform safety blocked: %s"));
        assert!(script.contains("/tmp/mister-magik/fs-fault-session"));
        assert!(script.contains("/tmp/mister-magik/fs-fault-launcher.env"));
        assert!(script.contains("/tmp/mister-magik/fs-fault.json"));
        assert!(script.contains("platform upload hash mismatch:"));
        assert!(
            script.contains("platform snapshot missing: /media/fat/MiSTer.ini.platform-rollback")
        );
        assert!(!script.contains("cp -p '/media/fat/MiSTer_MagiKDev'"));
        let cleanup = platform_cleanup_script();
        assert!(
            cleanup.find("fs-fault.json").unwrap()
                < cleanup.find("MiSTer_MagiKDev.rollback").unwrap()
        );
        assert!(platform_rollback_script().contains("MiSTer.ini.platform-rollback"));
        let snapshot = platform_snapshot_script();
        assert!(snapshot.contains("trap cleanup EXIT INT TERM"));
        assert!(
            snapshot.find("platform safety blocked").unwrap() < snapshot.find("cleanup()").unwrap()
        );
        assert!(snapshot.contains("MiSTer_MagiKDev.rollback"));
        assert!(snapshot.contains(
            "mkdir -p '/media/fat/mister-magik-dev/fpga'; : > '/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf.rollback-missing'"
        ));
        assert!(
            snapshot
                .find("rm -f '/media/fat/MiSTer_MagiKDev.rollback'")
                .unwrap()
                < snapshot
                    .find(
                        "cp -p '/media/fat/MiSTer_MagiKDev' '/media/fat/MiSTer_MagiKDev.rollback'"
                    )
                    .unwrap()
        );
        fs::remove_dir_all(stage).unwrap();
    }

    struct ScriptedPlatformDeployRemote {
        inventory: String,
        events: RefCell<Vec<String>>,
        fail_command_containing: Option<&'static str>,
        fail_upload: bool,
    }

    impl ScriptedPlatformDeployRemote {
        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    impl DeployRemote for ScriptedPlatformDeployRemote {
        fn exec(&self, command: &str) -> Result<ExecOutput> {
            self.events.borrow_mut().push(format!("exec {command}"));
            if self
                .fail_command_containing
                .is_some_and(|needle| command.contains(needle))
            {
                return Ok(ExecOutput {
                    rc: 9,
                    stdout: "scripted failure".to_string(),
                    stderr: String::new(),
                });
            }
            Ok(ExecOutput {
                rc: 0,
                stdout: if command.starts_with("set -eu; if test -f") {
                    self.inventory.clone()
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }

        fn put(&self, local: &Path, remote: &str) -> Result<()> {
            self.events
                .borrow_mut()
                .push(format!("put {} {remote}", local.display()));
            if self.fail_upload {
                return Err("scripted platform upload failure".into());
            }
            Ok(())
        }
    }

    fn platform_stage(label: &str) -> (PathBuf, PlatformDeployTransaction) {
        let stage = temp_path(label);
        for (relative, _) in PLATFORM_DEPLOY_FILES {
            let path = stage.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, relative.as_bytes()).unwrap();
        }
        let transaction = PlatformDeployTransaction::validate(&stage).unwrap();
        (stage, transaction)
    }

    fn platform_inventory(
        transaction: &PlatformDeployTransaction,
        changed_or_missing: &[(&str, bool)],
    ) -> String {
        transaction
            .files
            .iter()
            .map(|file| {
                match changed_or_missing
                    .iter()
                    .find(|(remote, _)| *remote == file.remote)
                {
                    Some((_, true)) => format!("missing  {}", file.remote),
                    Some((_, false)) => format!("{}  {}", "0".repeat(64), file.remote),
                    None => format!("{}  {}", file.sha256, file.remote),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn scripted_platform_remote(inventory: String) -> ScriptedPlatformDeployRemote {
        ScriptedPlatformDeployRemote {
            inventory,
            events: RefCell::new(Vec::new()),
            fail_command_containing: None,
            fail_upload: false,
        }
    }

    #[test]
    fn platform_deploy_skips_every_matching_remote_artifact() {
        let (stage, transaction) = platform_stage("platform-stage-unchanged");
        let remote = scripted_platform_remote(platform_inventory(&transaction, &[]));

        let report = transaction.run_with(&remote).unwrap();

        assert_eq!(report.changed_files, 0);
        assert_eq!(report.skipped_files, PLATFORM_DEPLOY_FILES.len());
        assert_eq!(report.transferred_bytes, 0);
        assert_eq!(remote.events().len(), 1, "only inventory should run");
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn platform_deploy_uploads_only_changed_files_and_activates_manifest_last() {
        let (stage, transaction) = platform_stage("platform-stage-incremental");
        let gui = "/media/fat/mister-magik-dev/mister-magik-fb";
        let manifest = "/media/fat/mister-magik-dev/platform-v3.manifest";
        let remote = scripted_platform_remote(platform_inventory(
            &transaction,
            &[(gui, false), (manifest, false)],
        ));

        let report = transaction.run_with(&remote).unwrap();
        let events = remote.events();
        let uploads = events
            .iter()
            .filter(|event| event.starts_with("put "))
            .collect::<Vec<_>>();
        let activation = events.last().unwrap();

        assert_eq!(report.changed_files, 2);
        assert_eq!(uploads.len(), 2);
        assert!(
            uploads
                .iter()
                .any(|event| event.ends_with(&format!("{gui}.upload")))
        );
        assert!(
            uploads
                .iter()
                .any(|event| event.ends_with(&format!("{manifest}.upload")))
        );
        assert!(
            !events
                .iter()
                .any(|event| event.starts_with("put ") && event.contains("mame.sqlite3"))
        );
        assert!(!activation.contains("cp -p '/media/fat/mister-magik-dev/mame.sqlite3'"));
        assert!(
            activation.find(&format!("{gui}.upload")).unwrap()
                < activation.find(&format!("{manifest}.upload")).unwrap()
        );
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn platform_deploy_treats_missing_remote_artifact_as_changed() {
        let (stage, transaction) = platform_stage("platform-stage-missing-remote");
        let manager = "/media/fat/mister-magik-dev/mister-magik-manager";
        let remote = scripted_platform_remote(platform_inventory(&transaction, &[(manager, true)]));

        let report = transaction.run_with(&remote).unwrap();

        assert_eq!(report.changed_files, 1);
        assert!(remote.events().iter().any(
            |event| event.starts_with("put ") && event.ends_with(&format!("{manager}.upload"))
        ));
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn platform_deploy_rejects_invalid_inventory_before_upload() {
        let (stage, transaction) = platform_stage("platform-stage-invalid-inventory");
        let remote = scripted_platform_remote("invalid".to_string());

        let error = transaction.run_with(&remote).unwrap_err().to_string();

        assert!(error.contains("platform inventory returned"));
        assert!(
            !remote
                .events()
                .iter()
                .any(|event| event.starts_with("put "))
        );
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn platform_deploy_rejects_incomplete_stages() {
        let stage = temp_path("platform-stage-missing");
        fs::create_dir_all(&stage).unwrap();
        let error = PlatformDeployTransaction::validate(&stage)
            .unwrap_err()
            .to_string();
        assert!(error.contains("platform stage is missing mister-magik-fb"));
        fs::remove_dir_all(stage).unwrap();
    }

    #[test]
    fn delivery_smoke_keeps_unconditional_safety_checks_outside_presenter_policy() {
        let command = delivery_smoke_command("dev", &"a".repeat(64)).unwrap();
        for required in [
            "sha256sum",
            "pidof MiSTer_MagiKDev",
            "pidof mister-magik-fb",
            "\"scene\"",
            "\"effective_view\"",
            "\"return_screen\"",
            "status_sequence",
            "pid_before",
            "pid_after",
            "bits_per_pixel",
            "/media/fat/mister-magik/launcher.env",
            "/media/fat/mister-magik-dev/launcher.env",
            "/tmp/mister-magik/fs-fault-session",
        ] {
            assert!(
                command.contains(required),
                "missing smoke check: {required}"
            );
        }
        assert!(command.contains("test \"$sequence_after\" -gt \"$sequence_before\""));
        assert!(command.contains("delivery_smoke_failure_tsv"));
        assert!(command.contains("heartbeat_attempts\" -lt 10"));
        assert!(command.contains("smoke_check=launcher-pid-stable"));
        assert!(!command.contains("sleep 2"));
        assert!(!command.contains("latch-readiness-report"));
        assert!(!command.contains("mister_magik_scanout_slots"));
        let latch_health = delivery_health_command("dev").unwrap();
        assert!(latch_health.contains("latch-readiness-report"));
        assert!(latch_health.contains("mister_magik_scanout_slots"));
        assert!(latch_health.contains("delivery_health_failure_tsv"));
        assert!(latch_health.contains("health_check=latch-readiness"));
        assert!(validate_delivery_remote("/tmp/not-owned").is_err());
    }

    #[test]
    fn legacy_launcher_restart_cleanup_is_narrow_and_age_bounded() {
        let command = legacy_launcher_restart_cleanup_command();
        assert!(command.contains("$proc/fd/8"));
        assert!(command.contains("/tmp/mister-magik/command-operation.lock"));
        assert!(command.contains("mister_magik_restart_launcher"));
        assert!(command.contains("-ge 30"));
        assert!(command.contains("kill \"$pid\""));
    }

    #[test]
    fn delivery_smoke_retries_only_healthy_latch_startup_input() {
        let mut status = json!({
            "scene": "launcher",
            "present_backend": "fpga-vblank-latch-hidden",
            "present_status": "ok",
            "input_enabled": false,
        });
        assert!(delivery_status_waiting_for_input(&status));

        status["input_enabled"] = json!(true);
        assert!(!delivery_status_waiting_for_input(&status));
        status["input_enabled"] = json!(false);
        status["present_status"] = json!("compatibility");
        assert!(!delivery_status_waiting_for_input(&status));
    }

    #[test]
    fn diagnostic_facts_sample_launcher_heartbeat() {
        let command = diagnostic_facts_command();
        assert!(command.contains("status_sequence"));
        assert!(command.contains("launcher_heartbeat_advancing"));
        assert!(command.contains("scanout_ready"));
        assert!(command.contains("latch_ready"));
        assert!(command.contains("latch-readiness-report"));
        assert!(command.contains("pid_before"));
        assert!(command.contains("pid_after"));
        assert!(command.contains("test -r \"$status\""));
        assert!(command.contains("pid_before=; sequence_before=; pid_after=; sequence_after="));
    }

    #[test]
    fn crash_report_reads_are_confined_to_main_owned_report_files() {
        assert!(is_safe_crash_report_path(
            "/media/fat/mister-magik/crashes/report-main-1.json"
        ));
        assert!(is_safe_crash_report_path(
            "/media/fat/mister-magik-dev/crashes/report-main-1.json"
        ));
        assert!(!is_safe_crash_report_path(
            "/media/fat/mister-magik-dev/crashes/latest.json"
        ));
        assert!(!is_safe_crash_report_path(
            "/media/fat/mister-magik-dev/crashes/report-../secret.json"
        ));
    }

    #[test]
    fn release_recovery_requires_volatile_token_and_clears_every_arming_path() {
        let begin = release_begin_command();
        assert!(!begin.contains(";;"));
        assert!(begin.contains(RELEASE_SNAPSHOT));
        assert!(RELEASE_SNAPSHOT.starts_with("/media/fat/"));
        let rearm = release_rearm_token_command();
        assert!(rearm.contains(RELEASE_TOKEN));
        assert!(!rearm.contains(RELEASE_SNAPSHOT));
        let catalog = release_catalog_command();
        assert!(catalog.contains("pidof MiSTer_MagiKDev"));
        assert!(catalog.contains("root=/media/fat/mister-magik-dev"));
        let recovery = release_recovery_command();
        assert!(recovery.contains(RELEASE_TOKEN));
        assert!(recovery.contains("attended-non-network-recovery-confirmed"));
        let restore = release_restore_command();
        assert!(!restore.contains(";;"));
        assert!(restore.contains(RELEASE_SNAPSHOT));
        for path in [
            "/media/fat/mister-magik/launcher.env",
            "/media/fat/mister-magik-dev/launcher.env",
            "/tmp/mister-magik/fs-fault-launcher.env",
            "/tmp/mister-magik/fs-fault-session",
            "/tmp/mister-magik/fs-fault.json",
            "/media/fat/mister-magik/rebuild-on-next-boot",
            "/media/fat/mister-magik-dev/rebuild-on-next-boot",
            RELEASE_TOKEN,
        ] {
            assert!(restore.contains(path), "missing release cleanup: {path}");
        }
        assert!(release_handoff_command().contains("/dev/MiSTer_cmd"));
        assert_eq!(
            RELEASE_DISPLAY_MODES
                .iter()
                .map(|mode| (
                    mode.label,
                    mode.video_mode,
                    mode.framebuffer,
                    mode.stride_bytes
                ))
                .collect::<Vec<_>>(),
            vec![
                ("wide-768", "10", "1366x768", 2736),
                ("tall-1536", "13", "1024x768", 2048),
                ("pixel-repeat-1440", "14", "1280x720", 2560),
                ("hd-1080", "8", "960x540", 1920),
                ("hd-720", "0", "1280x720", 2560),
                ("custom-1200", "1920,1200,60", "960x600", 1920),
            ]
        );
        for mode in RELEASE_DISPLAY_MODES {
            let command = release_display_mode_command(mode);
            assert!(command.contains("bits_per_pixel"));
            assert!(command.contains("release_display_readiness_json"));
            assert!(command.contains("release_display_plan"));
            assert!(command.contains("release_display_latch"));
            assert!(command.contains("release_display_bpp"));
            assert!(command.contains("\"scanout_abi_version\":3"));
            assert!(command.contains(mode.output));
            assert!(command.contains(mode.framebuffer));
        }
    }

    #[test]
    fn diagnosis_repairs_only_owned_temporary_state() {
        let facts = diagnostic_facts_command();
        assert!(facts.contains("credentials_ready"));
        assert!(facts.contains("firmware_compatible"));
        assert!(facts.contains("reboot_unstable"));
        assert!(facts.contains("arming_files"));
        let repair = safe_repair_command();
        assert!(repair.contains("agent-benchmark.tsv"));
        assert!(repair.contains("realtime-frame-analytics"));
        assert!(repair.contains("screensaver-profile"));
        assert!(repair.contains("rm -f /media/fat/mister-magik/launcher.env"));
        assert!(repair.contains("/media/fat/mister-magik-dev/launcher.env"));
        assert!(repair.contains("/media/fat/mister-magik-dev/rebuild-on-next-boot"));
    }

    #[test]
    fn one_shot_recovery_clears_arming_and_refuses_known_reboot_instability() {
        let preflight = one_shot_recovery_preflight_command();
        assert!(preflight.contains("test ! -e /tmp/mister-magik/reboot-unstable"));
        assert!(preflight.contains("rm -f /media/fat/mister-magik/launcher.env"));
        assert!(preflight.contains("/media/fat/mister-magik-dev/launcher.env"));
        assert!(preflight.contains("/tmp/mister-magik/fs-fault-session"));
        assert!(preflight.ends_with("sync"));
    }

    #[test]
    #[should_panic(expected = "shell command fragments must not own sequence separators")]
    fn shell_sequence_rejects_fragment_owned_separators() {
        shell_sequence(["set -eu;", "true"]);
    }

    #[test]
    fn installed_screensaver_summary_requires_catalog_off_and_production_present() {
        let frame = |id: u64, wall_us: u64| {
            let mut frame = json!({
                "frame": id,
                "screensaver_active": true,
                "wall_us": wall_us,
                "prepare_us": 1_000,
                "render_us": 5_000,
                "custom_draw_us": 1_000,
                "present_us": 500,
                "cpu_prepare_us": 100,
                "cpu_render_us": 500,
                "cpu_custom_draw_us": 100,
                "cpu_vsync_us": 10,
                "cpu_frame_tail_us": 20,
                "process_cpu_us": 730,
                "completion_monotonic_us": id * 16_667,
                "vsync_period_us": 16_667,
                "vsync_miss_streak": 0,
                "vsync_stale_hits": 0,
                "vsync_wait_start_age_us": 0,
                "vsync_accepted_hit_age_us": 0,
                "vsync_source": "vsync",
                "main_present_status": "ok",
                "main_present_sequence": id,
                "main_present_active_sequence": id,
                "main_present_pending": false,
                "main_present_flip_count": id + 10,
                "main_present_drop_count": 2,
            });
            let remaining = json!({
                "status_write_due": false,
                "runtime_status_write_us": 0,
                "clock_update_due": false,
                "clock_update_us": 0,
                "screensaver_sampling_profile": "legacy-half",
                "screensaver_archive_poll_us": 0,
                "screensaver_card_adopt_us": 0,
                "screensaver_parade_advance_us": 100,
                "screensaver_background_us": 100,
                "screensaver_draw_order_us": 100,
                "screensaver_tile_blit_us": 4_000,
                "screensaver_raster_held_cards": u64::from(id == 4),
                "screensaver_raster_moved_cards": 4,
                "screensaver_raster_hold_layer_mask": u64::from(id == 4),
                "screensaver_raster_visible_layer_mask": 31,
                "screensaver_renderer": "parade",
            });
            let particle = json!({
                "particle_preset": "capacity",
                "particle_phase": "static",
                "particle_simulation_backend": "scalar",
                "particle_projection_backend": "scalar-exact",
                "particle_count": 0,
                "particle_visible": 0,
                "particle_simulation_us": 0,
                "particle_simulation_cpu_us": 0,
                "particle_projection_us": 0,
                "particle_projection_cpu_us": 0,
                "particle_preparation_wait_us": 0,
                "particle_prepared_frame_age_us": 0,
                "particle_lookahead_mismatch_count": 0,
                "particle_preparation_queue_depth": 0,
                "particle_worker_wake_latency_us": 0,
                "particle_clear_us": 0,
                "particle_clear_cpu_us": 0,
                "particle_raster_us": 0,
                "particle_raster_cpu_us": 0,
                "particle_render_cpu_start": 0,
                "particle_render_cpu_end": 0,
                "particle_voluntary_context_switches": 0,
                "particle_involuntary_context_switches": 0,
                "particle_pmu_available": false,
                "particle_pmu_cycles": 0,
                "particle_pmu_instructions": 0,
                "particle_pmu_cache_references": 0,
                "particle_pmu_cache_misses": 0,
                "particle_pmu_branch_instructions": 0,
                "particle_pmu_branch_misses": 0,
                "particle_rotation_y_millidegrees": 0,
                "particle_simulation_bytes": 0,
                "particle_renderer_scratch_bytes": 0
            });
            frame
                .as_object_mut()
                .expect("screensaver evidence must be an object")
                .extend(
                    remaining
                        .as_object()
                        .expect("remaining screensaver evidence must be an object")
                        .clone(),
                );
            frame
                .as_object_mut()
                .expect("screensaver evidence must be an object")
                .extend(
                    particle
                        .as_object()
                        .expect("particle evidence must be an object")
                        .clone(),
                );
            frame["screensaver_render_ahead_sequence"] = json!(id);
            frame["screensaver_render_ahead_queue_depth"] = json!(1);
            frame["screensaver_render_ahead_frame_age_us"] = json!(400);
            frame["screensaver_render_ahead_render_wall_us"] = json!(4_500);
            frame["screensaver_render_ahead_render_cpu_us"] = json!(4_300);
            frame["screensaver_render_ahead_starvation_count"] = json!(0);
            frame["screensaver_render_ahead_superseded_frames"] = json!(0);
            frame["screensaver_render_ahead_reused_frames"] = json!(0);
            frame["screensaver_render_ahead_cancelled"] = json!(false);
            frame["status_publish_mode"] = json!("async");
            frame["status_enqueue_us"] = json!(0);
            frame["status_worker_write_us"] = json!(24_000);
            frame["status_replaced_count"] = json!(0);
            frame["status_submitted_sequence"] = json!(id);
            frame["status_written_sequence"] = json!(id);
            frame["status_worker_errors"] = json!(0);
            frame
        };
        let telemetry = [json!({
            "launcher": {
                "screensaver_profile_state": "active",
                "status_publish_mode": "async",
                "status_submitted_sequence": 9,
                "status_written_sequence": 9,
                "status_replaced_count": 0,
                "status_worker_write_us": 24_000,
                "status_worker_errors": 0,
                "catalog_refresh_policy": "off",
                "catalog_worker_enabled": false,
                "present_backend": "fpga-vblank-latch-hidden",
                "present_status": "ok",
                "latch_drop_count": 2,
                "frame_budget": {
                    "error_total": 0,
                    "recent_frames": [
                        frame(1, 500_000),
                        frame(2, 40_000),
                        frame(3, 20_000),
                        frame(4, 16_000),
                        frame(5, 16_667),
                        frame(6, 99_000)
                    ]
                }
            }
        })];
        let summary = summarize_screensaver_telemetry(
            1,
            &telemetry,
            json!({
                "state": "complete",
                "duration_secs": 1.0,
                "first_frame": 1,
                "last_frame": 5
            }),
        )
        .unwrap();
        assert_eq!(summary["startup"]["ignored_frames"], 3);
        assert_eq!(summary["captured_frames"], 6);
        assert_eq!(summary["measurement_window"]["last_frame"], 5);
        assert_eq!(summary["startup"]["max_wall_us"], 500_000);
        assert_eq!(summary["startup"]["over_budget_frames"], 3);
        assert_eq!(summary["steady_state"]["frames"], 2);
        assert_eq!(summary["steady_state"]["average_fps"], 2.0);
        assert_eq!(summary["steady_state"]["over_budget_frames"], 0);
        assert_eq!(summary["steady_state"]["presentation_failures"], json!([]));
        assert_eq!(summary["latch_drop_delta"], 0);
        assert_eq!(summary["raster_cadence"]["held_frames"], 1);
        assert_eq!(summary["render_ahead"]["starvation_count"], 0);
        assert_eq!(summary["qualification"]["qualified"], true);
        let populated_cpu =
            summary["populated_window"]["cpu_utilization"]["launcher_process_pct_of_one_core"]
                .as_f64()
                .unwrap();
        assert!((populated_cpu - 4.38).abs() < 0.01);
        let report = screensaver_benchmark_report(&json!({
            "manifest": {"magik_revision": "test-revision"},
            "display": {"final_mode": "hdmi-1280x720p60"},
            "runs": [summary.clone()],
        }))
        .unwrap();
        assert!(report.contains("Presentation failures"));
        assert!(report.contains("Visible raster holds"));
        assert!(report.contains("Render-ahead pipeline"));
        assert!(report.contains("Normalized CPU utilization"));
        assert!(report.contains("Qualification"));
        let mut repeated = summary.clone();
        repeated["steady_state"]["physical_refresh"]["repeated_refreshes"] = json!(1);
        assert!(
            screensaver_qualification_failures(&repeated)
                .iter()
                .any(|failure| failure["kind"] == "repeated-refreshes")
        );
        let mut reused = summary.clone();
        reused["render_ahead"]["reused_frames"] = json!(1);
        assert!(
            screensaver_qualification_failures(&reused)
                .iter()
                .any(|failure| failure["kind"] == "render-ahead-reuse")
        );
        let mut starved = summary.clone();
        starved["render_ahead"]["starvation_count"] = json!(1);
        assert!(
            screensaver_qualification_failures(&starved)
                .iter()
                .any(|failure| failure["kind"] == "render-ahead-starvation")
        );
        let mut sequence_gap = summary.clone();
        sequence_gap["steady_state"]["presentation_failures"] = json!([{
            "kind": "render-sequence-gap",
            "frame": 5,
            "previous": 3,
            "current": 5
        }]);
        assert!(
            screensaver_qualification_failures(&sequence_gap)
                .iter()
                .any(|failure| failure["kind"] == "presentation-failures")
        );
        let mut slow_worker = summary.clone();
        slow_worker["populated_window"]["render_ahead"]["render_wall_us"]["p99"] = json!(16_668);
        assert!(
            screensaver_qualification_failures(&slow_worker)
                .iter()
                .any(|failure| failure["kind"] == "worker-p99-over-refresh-period")
        );
        assert!(
            summarize_screensaver_telemetry(
                1,
                &telemetry,
                json!({
                    "state": "complete",
                    "duration_secs": 1.0,
                    "first_frame": 1,
                    "last_frame": 7
                })
            )
            .is_err()
        );

        let mut invalid = telemetry[0].clone();
        invalid["launcher"]["catalog_refresh_policy"] = json!("auto");
        assert!(
            summarize_screensaver_telemetry(
                1,
                &[invalid],
                json!({
                    "state": "complete",
                    "duration_secs": 1.0,
                    "first_frame": 1,
                    "last_frame": 5
                })
            )
            .is_err()
        );
    }

    #[test]
    fn failed_screensaver_qualification_retains_summary_and_report() {
        let output_dir = std::env::temp_dir().join(format!(
            "mister-magik-failed-screensaver-qualification-{}",
            std::process::id()
        ));
        fs::create_dir_all(&output_dir).unwrap();
        let summary = json!({
            "manifest": {"magik_revision": "test-revision"},
            "display": {"final_mode": "hdmi-1280x720p60"},
            "runs": [{
                "run": 1,
                "qualification": {
                    "qualified": false,
                    "failures": [{"kind": "render-ahead-starvation", "count": 1}]
                }
            }]
        });

        let error = persist_and_qualify_screensaver_benchmark(&output_dir, &summary)
            .unwrap_err()
            .to_string();

        assert!(error.contains("evidence retained"));
        assert_eq!(
            serde_json::from_str::<Value>(
                &fs::read_to_string(output_dir.join("summary.json")).unwrap()
            )
            .unwrap(),
            summary
        );
        assert!(
            fs::read_to_string(output_dir.join("report.md"))
                .unwrap()
                .contains("Qualification")
        );
        fs::remove_file(output_dir.join("summary.json")).unwrap();
        fs::remove_file(output_dir.join("report.md")).unwrap();
        fs::remove_dir(output_dir).unwrap();
    }

    #[test]
    fn periodic_timing_analysis_recovers_a_sixty_frame_signal() {
        let values = (0..1_800)
            .map(|frame| {
                let phase = std::f64::consts::TAU * frame as f64 / 60.0;
                12_000.0 + 500.0 * phase.sin()
            })
            .collect::<Vec<_>>();
        let signal = periodic_signal(&values, 16_667);
        let period = signal["period_frames"].as_f64().unwrap();
        let amplitude = signal["amplitude_us"].as_f64().unwrap();
        assert!((period - 60.0).abs() <= 0.25);
        assert!(amplitude > 450.0);
    }

    #[test]
    fn presentation_sequence_continuity_accepts_u16_wrap_only() {
        assert!(presentation_sequence_is_contiguous(41, 42));
        assert!(presentation_sequence_is_contiguous(u16::MAX, 1));
        assert!(!presentation_sequence_is_contiguous(u16::MAX, 0));
        assert!(!presentation_sequence_is_contiguous(41, 43));
        assert!(!presentation_sequence_is_contiguous(41, 41));
    }

    fn physical_frame(frame: u64, completion_us: u64, flip_count: u16) -> Value {
        json!({
            "frame": frame,
            "completion_monotonic_us": completion_us,
            "main_present_flip_count": flip_count,
        })
    }

    fn physical_summary(frames: &[Value], period_us: u64) -> Result<Value> {
        let references = frames.iter().collect::<Vec<_>>();
        physical_refresh_summary(1, &references, period_us)
    }

    #[test]
    fn physical_refresh_summary_detects_repeats_despite_contiguous_submissions() {
        let frames = [
            physical_frame(100, 1_000_000, 100),
            physical_frame(101, 1_016_667, 101),
            physical_frame(102, 1_050_001, 102),
        ];
        let summary = physical_summary(&frames, 16_667).unwrap();
        assert_eq!(summary["expected_refresh_intervals"], 3);
        assert_eq!(summary["unique_latch_flips"], 2);
        assert_eq!(summary["repeated_refreshes"], 1);
        assert_eq!(
            summary["long_completion_intervals"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn physical_refresh_summary_accepts_exact_sixty_hz_and_counter_wrap() {
        let frames = [
            physical_frame(1, 1_000_000, u16::MAX - 1),
            physical_frame(2, 1_016_667, u16::MAX),
            physical_frame(3, 1_033_334, 0),
            physical_frame(4, 1_050_001, 1),
        ];
        let summary = physical_summary(&frames, 16_667).unwrap();
        assert_eq!(summary["expected_refresh_intervals"], 3);
        assert_eq!(summary["unique_latch_flips"], 3);
        assert_eq!(summary["repeated_refreshes"], 0);
        assert_eq!(summary["long_completion_intervals"], json!([]));
    }

    #[test]
    fn physical_refresh_summary_does_not_accumulate_nominal_clock_error() {
        let frames = (0..1_827)
            .map(|index| {
                physical_frame(
                    index,
                    1_000_000 + index * 16_667,
                    u16::try_from(index).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let summary = physical_summary(&frames, 16_662).unwrap();
        assert_eq!(summary["expected_refresh_intervals"], 1_826);
        assert_eq!(summary["unique_latch_flips"], 1_826);
        assert_eq!(summary["repeated_refreshes"], 0);
        assert_eq!(summary["long_completion_intervals"], json!([]));
    }

    #[test]
    fn physical_refresh_summary_rejects_missing_timestamps() {
        let frames = [physical_frame(1, 1_000_000, 1), physical_frame(2, 0, 2)];
        assert!(physical_summary(&frames, 16_667).is_err());
    }

    #[test]
    fn physical_refresh_summary_records_long_intervals() {
        let frames = [
            physical_frame(1, 1_000_000, 1),
            physical_frame(2, 1_025_001, 2),
        ];
        let summary = physical_summary(&frames, 16_667).unwrap();
        assert_eq!(
            summary["long_completion_intervals"],
            json!([{"frame": 2, "interval_us": 25_001}])
        );
    }

    #[test]
    fn physical_refresh_summary_exposes_pending_zero_then_two_flip_pattern() {
        let frames = [
            physical_frame(953, 1_000_000, 3416),
            physical_frame(954, 1_016_667, 3416),
            physical_frame(955, 1_050_001, 3418),
        ];
        let summary = physical_summary(&frames, 16_667).unwrap();
        assert_eq!(summary["expected_refresh_intervals"], 3);
        assert_eq!(summary["unique_latch_flips"], 2);
        assert_eq!(summary["repeated_refreshes"], 1);
    }

    fn particle_evidence_frame(index: u64, render_wall_us: u64) -> Value {
        let frame = index + 1;
        let phase = match index % 600 {
            0..=179 => "static",
            180..=299 => "form",
            300..=479 => "hold",
            _ => "disperse",
        };
        let mut evidence = json!({
            "frame": frame,
            "wall_us": render_wall_us,
            "prepare_us": 100,
            "render_us": render_wall_us,
            "custom_draw_us": 0,
            "present_us": 100,
            "cpu_prepare_us": 80,
            "cpu_render_us": render_wall_us - 100,
            "cpu_custom_draw_us": 0,
            "cpu_vsync_us": 10,
            "cpu_frame_tail_us": 10,
            "process_cpu_us": render_wall_us,
            "completion_monotonic_us": frame * 16_667,
            "vsync_period_us": 16_667,
            "vsync_miss_streak": 0,
            "vsync_stale_hits": 0,
            "vsync_wait_start_age_us": 0,
            "vsync_accepted_hit_age_us": 0,
            "vsync_source": "vsync",
            "main_present_status": "ok",
            "main_present_copy_path": "external-direct",
            "main_present_sequence": frame,
            "main_present_active_sequence": frame,
            "main_present_pending": false,
            "main_present_flip_count": frame,
            "main_present_drop_count": 0,
            "status_write_due": false,
            "runtime_status_write_us": 0,
            "status_enqueue_us": 0,
            "status_worker_write_us": 0,
            "status_replaced_count": 0,
            "status_submitted_sequence": frame,
            "status_written_sequence": frame,
            "status_worker_errors": 0,
            "status_publish_mode": "async",
        });
        let remaining = json!({
            "clock_update_due": false,
            "clock_update_us": 0,
            "screensaver_active": true,
            "screensaver_renderer": "particle-magik",
            "screensaver_sampling_profile": "full",
            "screensaver_archive_poll_us": 0,
            "screensaver_card_adopt_us": 0,
            "screensaver_parade_advance_us": 0,
            "screensaver_background_us": 0,
            "screensaver_draw_order_us": 0,
            "screensaver_tile_blit_us": 0,
            "screensaver_raster_held_cards": 0,
            "screensaver_raster_moved_cards": 0,
            "screensaver_raster_hold_layer_mask": 0,
            "screensaver_raster_visible_layer_mask": 0,
            "screensaver_render_ahead_sequence": frame,
            "screensaver_render_ahead_queue_depth": 1,
            "screensaver_render_ahead_frame_age_us": 100,
            "screensaver_render_ahead_render_wall_us": render_wall_us,
            "screensaver_render_ahead_render_cpu_us": render_wall_us - 100,
            "screensaver_render_ahead_starvation_count": 0,
            "screensaver_render_ahead_superseded_frames": 0,
            "screensaver_render_ahead_reused_frames": 0,
            "screensaver_render_ahead_cancelled": false,
        });
        let particle = json!({
            "particle_preset": "capacity",
            "particle_phase": phase,
            "particle_simulation_backend": "armv7-neon",
            "particle_projection_backend": "armv7-neon-corrected",
            "particle_count": 65_536,
            "particle_visible": 65_536,
            "particle_simulation_us": 2_000,
            "particle_simulation_cpu_us": 1_900,
            "particle_projection_us": 2_500,
            "particle_projection_cpu_us": 2_400,
            "particle_preparation_wait_us": 50,
            "particle_prepared_frame_age_us": 75,
            "particle_lookahead_mismatch_count": 0,
            "particle_preparation_queue_depth": 1,
            "particle_worker_wake_latency_us": 20,
            "particle_clear_us": 200,
            "particle_clear_cpu_us": 180,
            "particle_raster_us": 5_000,
            "particle_raster_cpu_us": 4_800,
            "particle_render_cpu_start": 0,
            "particle_render_cpu_end": 0,
            "particle_voluntary_context_switches": 0,
            "particle_involuntary_context_switches": u64::from(index == 100),
            "particle_pmu_available": true,
            "particle_pmu_cycles": 10_000,
            "particle_pmu_instructions": 8_000,
            "particle_pmu_cache_references": 1_000,
            "particle_pmu_cache_misses": 100,
            "particle_pmu_branch_instructions": 500,
            "particle_pmu_branch_misses": 25,
            "particle_rotation_y_millidegrees": 0,
            "particle_simulation_bytes": 2_162_688,
            "particle_renderer_scratch_bytes": 786_432
        });
        evidence
            .as_object_mut()
            .expect("particle evidence must be an object")
            .extend(
                remaining
                    .as_object()
                    .expect("remaining particle evidence must be an object")
                    .clone(),
            );
        evidence
            .as_object_mut()
            .expect("particle evidence must be an object")
            .extend(
                particle
                    .as_object()
                    .expect("particle fields must be an object")
                    .clone(),
            );
        evidence
    }

    fn particle_telemetry(render_wall_us: u64) -> Vec<Value> {
        let frames = (0..604)
            .map(|index| particle_evidence_frame(index, render_wall_us))
            .collect::<Vec<_>>();
        vec![json!({
            "launcher": {
                "present_backend": "fpga-vblank-latch-hidden",
                "frame_budget": {"recent_frames": frames}
            }
        })]
    }

    #[test]
    fn particle_trial_requires_every_phase_and_the_render_reserve() {
        let passing = summarize_particle_trial(
            "capacity",
            65_536,
            12,
            "search",
            "telemetry.jsonl",
            &particle_telemetry(10_000),
        );
        assert_eq!(passing["qualified"], true);
        assert_eq!(passing["memory"]["simulation_bytes_per_particle"], 33);
        assert_eq!(passing["memory"]["renderer_scratch_bytes_per_particle"], 12);
        for phase in ["static", "form", "hold", "disperse"] {
            assert!(passing["phase_timing"][phase]["frames"].as_u64().unwrap() > 0);
            assert_eq!(
                passing["phase_timing"][phase]["simulation_descheduled_mean_us"],
                100.0
            );
            assert_eq!(
                passing["phase_timing"][phase]["projection_descheduled_mean_us"],
                100.0
            );
        }
        assert_eq!(passing["scheduler"]["involuntary_context_switches"], 1);
        assert_eq!(passing["scheduler"]["cpu_migrations"], 0);
        assert_eq!(passing["simulation_backends"], json!(["armv7-neon"]));
        assert_eq!(
            passing["projection_backends"],
            json!(["armv7-neon-corrected"])
        );
        assert_eq!(passing["pmu"]["available_frames"], 601);
        assert_eq!(passing["pmu"]["instructions_per_cycle"], 0.8);
        assert_eq!(passing["pmu"]["cache_miss_pct"], 10.0);
        assert_eq!(passing["pmu"]["branch_miss_pct"], 5.0);

        let deadline = summarize_particle_trial(
            "capacity",
            65_536,
            12,
            "search",
            "telemetry.jsonl",
            &particle_telemetry(15_917),
        );
        assert_eq!(deadline["qualified"], false);
        assert!(
            deadline["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failure| {
                    failure.get("kind").and_then(Value::as_str) == Some("render-deadline")
                })
        );
    }

    #[test]
    fn particle_capture_uses_the_newest_matching_frame() {
        let telemetry = vec![json!({
            "launcher": {
                "frame_budget": {
                    "recent_frames": [
                        {
                            "frame": 10,
                            "screensaver_active": true,
                            "screensaver_renderer": "particle-magik",
                            "particle_preset": "visual",
                            "particle_count": 16_384,
                            "particle_phase": "hold",
                            "particle_rotation_y_millidegrees": 45_000
                        },
                        {
                            "frame": 11,
                            "screensaver_active": true,
                            "screensaver_renderer": "particle-magik",
                            "particle_preset": "visual",
                            "particle_count": 16_384,
                            "particle_phase": "hold",
                            "particle_rotation_y_millidegrees": 75_000
                        }
                    ]
                }
            }
        })];
        assert!(!particle_capture_state_seen(
            &telemetry,
            16_384,
            "hold",
            30_000..=60_000
        ));
        assert!(particle_capture_state_seen(
            &telemetry,
            16_384,
            "hold",
            70_000..=80_000
        ));
    }

    #[test]
    fn firework_capture_ignores_only_undeclared_startup_renderer() {
        let frames = [
            json!({"screensaver_renderer": ""}),
            json!({"screensaver_renderer": "particle-demos"}),
        ];
        let references = frames.iter().collect::<Vec<_>>();
        assert_eq!(
            first_declared_screensaver_renderer(&references),
            Some("particle-demos")
        );

        let unexpected = [
            json!({"screensaver_renderer": ""}),
            json!({"screensaver_renderer": "other-renderer"}),
            json!({"screensaver_renderer": "particle-demos"}),
        ];
        let unexpected_references = unexpected.iter().collect::<Vec<_>>();
        assert_eq!(
            first_declared_screensaver_renderer(&unexpected_references),
            Some("other-renderer")
        );
    }

    #[test]
    fn particle_search_refines_to_1024_particle_precision() {
        assert_eq!(particle_refinement_count(0, 524_288), Some(262_144));
        assert_eq!(particle_refinement_count(131_072, 262_144), Some(196_608));
        assert_eq!(particle_refinement_count(196_608, 197_632), None);
    }

    #[test]
    fn particle_confirmation_falls_back_at_1024_particle_precision() {
        assert_eq!(particle_confirmation_backoff(18_432), Some(17_408));
        assert_eq!(particle_confirmation_backoff(2_048), Some(1_024));
        assert_eq!(particle_confirmation_backoff(1_024), None);
    }

    #[test]
    fn installed_benchmark_capability_accepts_the_runtime_log_prefix() {
        let capability = last_json_line(
            "mister-magik-fb [benchmark-capabilities] (arch=arm)\n\
             {\"screensaver-pprof-v1\":true,\"screensaver-frame-evidence-v3\":true}\n",
        )
        .unwrap();

        assert_eq!(capability["screensaver-pprof-v1"], true);
        assert_eq!(capability["screensaver-frame-evidence-v3"], true);
        assert!(last_json_line("no structured report").is_none());
    }

    #[test]
    fn installed_search_waits_only_for_catalog_schema_publication() {
        let pending = ExecOutput {
            rc: 1,
            stdout: String::new(),
            stderr: "search benchmark failed: no such table: game_search_fts".to_string(),
        };
        let unrelated = ExecOutput {
            rc: 1,
            stdout: String::new(),
            stderr: "search benchmark failed: disk I/O error".to_string(),
        };
        let unpublished = ExecOutput {
            rc: 1,
            stdout: "search benchmark failed: read-manifest: no valid manifest slot".to_string(),
            stderr: String::new(),
        };

        assert!(search_benchmark_waits_for_catalog(&pending));
        assert!(search_benchmark_waits_for_catalog(&unpublished));
        assert!(!search_benchmark_waits_for_catalog(&unrelated));
    }

    #[test]
    fn catalog_lifecycle_runtime_is_fully_isolated_from_production_data() {
        let refresh = catalog_lifecycle_runtime_command("library-refresh");
        assert!(refresh.contains("MISTER_SHARDED_CATALOG_DIR="));
        assert!(refresh.contains("MISTER_LIBRARY_SQLITE="));
        assert!(refresh.contains("MISTER_ARCADE_BOOTSTRAP_INDEX="));
        assert!(refresh.contains(CATALOG_LIFECYCLE_REMOTE_DIR));
        assert!(
            !refresh
                .contains("MISTER_SHARDED_CATALOG_DIR='/media/fat/mister-magik-dev/catalog-v3'")
        );
        assert!(catalog_lifecycle_cleanup_command().contains(CATALOG_LIFECYCLE_REMOTE_DIR));
        let affinity = catalog_lifecycle_affinity_command();
        assert!(affinity.contains("Cpus_allowed_list"));
        assert!(affinity.contains("/proc/$pid/task/*"));
        assert!(!affinity.contains("ps -eo"));
        let env = catalog_lifecycle_launcher_env();
        assert!(env.iter().any(|(key, value)| {
            key == "MISTER_CATALOG_DIAGNOSTICS_DIR"
                && value.starts_with(CATALOG_LIFECYCLE_REMOTE_DIR)
        }));
        assert!(
            env.iter()
                .filter(|(key, _)| key.starts_with("MISTER_"))
                .all(|(_, value)| !value.starts_with("/media/fat/mister-magik"))
        );
    }

    #[test]
    fn catalog_lifecycle_keeps_scripted_input_active_past_completion_deadline() {
        let script = catalog_lifecycle_input_script();
        assert_eq!(script.matches("wait:600").count(), 150);
        assert!(script.starts_with("down,up,"));
        assert!(script.len() < 4_096);
    }

    #[test]
    fn catalog_lifecycle_inspection_preserves_per_system_counts() {
        let parsed = parse_catalog_lifecycle_inspect(
            "catalog_v3_summary_tsv\tvalid=1\tschema=1\tgeneration=7\tsystems=2\ttotal_games=44\n\
             catalog_v3_system_tsv\tsystem=atari2600\trole=console\tgeneration=7\tregistry_games=3\tshard_games=3\n\
             catalog_v3_system_tsv\tsystem=arcade\trole=arcade\tgeneration=7\tregistry_games=41\tshard_games=41\n",
        )
        .unwrap();
        assert_eq!(parsed["valid"], true);
        assert_eq!(parsed["generation"], 7);
        assert_eq!(parsed["total_games"], 44);
        assert_eq!(parsed["systems"][0]["system"], "atari2600");
        assert_eq!(parsed["systems"][0]["games"], 3);
    }

    #[test]
    fn catalog_lifecycle_inspection_rejects_invalid_or_incomplete_output() {
        assert!(
            parse_catalog_lifecycle_inspect(
                "catalog_v3_summary_tsv\tvalid=0\tgeneration=7\ttotal_games=44"
            )
            .is_err()
        );
        assert!(
            parse_catalog_lifecycle_inspect("catalog_v3_summary_tsv\tvalid=1\tgeneration=7")
                .is_err()
        );
    }

    #[test]
    fn typed_operator_commands_own_platform_and_scene_safety() {
        for layout in [Layout::Development, Layout::Public] {
            let verify = installed_platform_verify_command(layout);
            assert!(verify.contains("platform-v3.manifest"));
            assert!(verify.contains("sha256sum"));
            assert!(verify.contains("mister-magik-manager"));
            assert!(verify.contains("manager_sha256"));
            assert!(verify.contains("scanout_module_sha256"));
            assert!(verify.contains("latch_rbf_sha256"));
        }
        assert!(release_arming_cleanup_command().contains("rebuild-on-next-boot"));
        assert!(!DeviceRequest::CaptureFramebuffer.label().contains("run"));
    }

    #[test]
    fn latch_diagnostics_cleanup_is_bounded_and_self_verifying() {
        let command = clear_latch_diagnostics_command();
        assert_eq!(
            command
                .matches("/media/fat/mister-magik/diagnostics/latch")
                .count(),
            3
        );
        assert_eq!(
            command
                .matches("/media/fat/mister-magik-dev/diagnostics/latch")
                .count(),
            3
        );
        assert!(command.contains("rm -rf /media/fat/mister-magik/diagnostics/latch"));
        assert!(command.contains("mkdir -p /media/fat/mister-magik/diagnostics/latch"));
        assert!(command.contains("-mindepth 1 -print -quit"));
        assert!(!command.contains("launcher.env"));
        assert!(!command.contains("rebuild-on-next-boot"));
    }

    #[test]
    fn discovery_access_denial_has_a_distinct_typed_failure() {
        assert_eq!(
            device_failure(
                "local-network access denied while discovering the MiSTer; rerun with network escalation"
            ),
            DeviceFailure::AccessDenied(
                "local-network access denied while discovering the MiSTer; rerun with network escalation"
                    .into()
            )
        );
    }

    #[test]
    fn diagnostics_bundle_exports_latest_support_reports() {
        let out = std::env::temp_dir().join(format!(
            "mister-magik-host-catalog-diagnostics-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        let bundle = json!({
            "catalog_failures": {
                "latest": {
                    "path": "/media/fat/mister-magik/diagnostics/catalog/latest.json",
                    "report": {
                        "schema": "mister-magik-catalog-failure-v1",
                        "report_id": "report-catalog-test"
                    }
                },
                "recent_paths": []
            },
            "catalog_progress": {
                "path": "/media/fat/mister-magik/diagnostics/catalog/progress-latest.json",
                "report": {
                    "schema": "mister-magik-catalog-progress-v1",
                    "episode_id": "progress-catalog-test"
                }
            },
            "latch_failure": {
                "path": "/media/fat/mister-magik/diagnostics/latch/latest.json",
                "report": {
                    "schema": "mister-magik-latch-failure-report-v1",
                    "episode_id": "report-latch-test"
                }
            },
        });

        write_diagnostics_bundle(&out, &bundle).unwrap();

        assert!(out.join("catalog-failures.json").exists());
        let latest: Value =
            serde_json::from_slice(&fs::read(out.join("catalog-failure-latest.json")).unwrap())
                .unwrap();
        assert_eq!(latest["schema"], "mister-magik-catalog-failure-v1");
        let progress: Value =
            serde_json::from_slice(&fs::read(out.join("catalog-progress-latest.json")).unwrap())
                .unwrap();
        assert_eq!(progress["schema"], "mister-magik-catalog-progress-v1");
        let latch: Value =
            serde_json::from_slice(&fs::read(out.join("latch-failure-latest.json")).unwrap())
                .unwrap();
        assert_eq!(latch["schema"], "mister-magik-latch-failure-report-v1");
        let _ = fs::remove_dir_all(out);
    }

    const MAME_1942_FIXTURE: &str = r#"<?xml version="1.0"?>
<mame build="0.288 (mame0288)" debug="no" mameconfig="10">
  <machine name="1942" sourcefile="capcom/1942.cpp">
    <description>1942 (Revision B)</description>
    <year>1984</year>
    <manufacturer>Capcom</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="2" coins="2">
      <control type="joy" player="1" buttons="2" ways="8" />
      <control type="joy" player="2" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
  <machine name="1942a" sourcefile="capcom/1942.cpp" cloneof="1942" romof="1942">
    <description>1942 (Revision A)</description>
    <year>1984</year>
    <manufacturer>Capcom</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="2" coins="2">
      <control type="joy" player="1" buttons="2" ways="8" />
      <control type="joy" player="2" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
  <machine name="1942p" sourcefile="capcom/1942.cpp" cloneof="1942" romof="1942">
    <description>1942 (Tecfri PCB, bootleg?)</description>
    <year>1984</year>
    <manufacturer>bootleg</manufacturer>
    <display tag="screen" type="raster" rotate="270" width="256" height="224" refresh="59.637405" />
    <input players="1" coins="2">
      <control type="joy" buttons="2" ways="8" />
    </input>
    <driver status="good" emulation="good" savestate="supported" />
  </machine>
</mame>
"#;
}
