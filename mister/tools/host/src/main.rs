// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_tool::transport::{
    BenchmarkScenario, ColdBenchmarkScenario, DeviceFailure, DeviceOperations, DeviceRequest,
    DeviceResponse, Layout, MainSelection,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ssh2::Session;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod agent_client;
mod crt_qualification;
mod discovery;
mod media;
mod remote;

use agent_client::{
    AGENT_PORT, AgentEndpoint, agent_binary_request_bounded, agent_request, agent_request_at,
    agent_request_with_liveness, agent_stream_request_reader, agent_token, agent_token_for_device,
    bootstrap_agent, bootstrap_agent_with, verify_agent_deploy_result,
};
use remote::{
    ConnectionConfig, ExecOutput, acknowledged_main_command, connect, connect_timed,
    connect_timed_with, connect_with, create_dir_command, exec, exec_failure_message, get, host,
    host_wait_diagnostics_with, launcher_restart_command, port_open, port_open_with, put,
    put_bytes, put_dir, remote_subcommand, remove_files_command, sftp_write_profile,
    shell_quote as sh, stream_command, tcp_probe_label, tcp_probe_label_port,
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
const MAIN_STATUS_REMOTE: &str = "/tmp/mister-magik/main-status.json";
const SLINT_STATUS_REMOTE: &str = "/tmp/mister-magik/status.json";
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
                remote_read(&session, "/media/fat/mister-magik-dev/platform-v2.manifest")
                    .unwrap_or_default()
            }
            DeviceRequest::SnapshotRuntime { remote } => {
                validate_delivery_remote(remote).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "runtime snapshot",
                    &format!(
                        "set -eu; rm -f {0}.delivery-rollback.tmp; cp -p {0} {0}.delivery-rollback.tmp; mv -f {0}.delivery-rollback.tmp {0}.delivery-rollback; sync",
                        sh(remote)
                    ),
                )
                .map_err(device_failure)?;
                "snapshotted".into()
            }
            DeviceRequest::DeployRuntime { local, remote } => {
                let session = connect(10).map_err(device_failure)?;
                deploy_magik_bin(&session, local, remote).map_err(device_failure)?;
                "deployed".into()
            }
            DeviceRequest::RollbackRuntime { remote } => {
                validate_delivery_remote(remote).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "runtime suspend for rollback",
                    &acknowledged_main_command("mister_magik_suspend"),
                )
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
                if let Err(error) = exec_checked(
                    &session,
                    "runtime rollback",
                    &format!(
                        "set -eu; test -f {0}.delivery-rollback; mv -f {0}.delivery-rollback {0}; chmod 755 {0}; sync",
                        sh(remote)
                    ),
                ) {
                    let _ = exec_checked(
                        &session,
                        "runtime resume after failed rollback",
                        &acknowledged_main_command("mister_magik_resume"),
                    );
                    return Err(DeviceFailure::RecoveryRequired(error.to_string()));
                }
                exec_checked(
                    &session,
                    "runtime resume after rollback",
                    &acknowledged_main_command("mister_magik_resume"),
                )
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
                "rolled-back".into()
            }
            DeviceRequest::CommitRuntime { remote } => {
                validate_delivery_remote(remote).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "runtime commit",
                    &format!("rm -f {}.delivery-rollback; sync", sh(remote)),
                )
                .map_err(device_failure)?;
                "committed".into()
            }
            DeviceRequest::DeployPlatform { stage } => {
                let transaction =
                    PlatformDeployTransaction::validate(stage).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                transaction.run(&session).map_err(device_failure)?;
                "staged".into()
            }
            DeviceRequest::SnapshotPlatform => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "platform snapshot", &platform_snapshot_script())
                    .map_err(device_failure)?;
                "snapshotted".into()
            }
            DeviceRequest::RollbackPlatform => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "platform rollback", &platform_rollback_script())
                    .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
                "rolled-back".into()
            }
            DeviceRequest::CommitPlatform => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(&session, "platform commit", &platform_cleanup_script())
                    .map_err(device_failure)?;
                "committed".into()
            }
            DeviceRequest::SelectMain(selection) => {
                let value = match selection {
                    MainSelection::Stock => "MiSTer",
                    MainSelection::Development => "MiSTer_MagiKDev",
                    MainSelection::Public => "MiSTer_MagiK",
                };
                let session = connect(10).map_err(device_failure)?;
                edit_remote_ini(&session, IniEdit::SelectMain(value.into()), false)
                    .map_err(device_failure)?;
                value.into()
            }
            DeviceRequest::RebootWait => {
                let session = connect(10).map_err(device_failure)?;
                issue_delivery_reboot(&session).map_err(device_failure)?;
                drop(session);
                if !wait_down_with(&config.connection, 40.0)
                    || wait_up_with(&config.connection, 120.0).map_err(device_failure)? != 0
                {
                    return Err(DeviceFailure::Unavailable(
                        "device did not complete its reboot transition".into(),
                    ));
                }
                "rebooted".into()
            }
            DeviceRequest::VerifyHealth(layout) => {
                let label = match layout {
                    Layout::Development => "dev",
                    Layout::Public => "public",
                };
                let command = delivery_health_command(label).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                exec_checked(&session, "delivery health", &command)
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                "healthy".into()
            }
            DeviceRequest::SmokeDelivery {
                layout,
                expected_sha256,
            } => {
                if expected_sha256.len() != 64
                    || !expected_sha256.chars().all(|ch| ch.is_ascii_hexdigit())
                {
                    return Err(DeviceFailure::InvalidRequest(
                        "expected SHA-256 is invalid".into(),
                    ));
                }
                let label = match layout {
                    Layout::Development => "dev",
                    Layout::Public => "public",
                };
                let command =
                    delivery_smoke_command(label, expected_sha256).map_err(device_failure)?;
                let session = connect(10).map_err(device_failure)?;
                wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                exec_checked(&session, "delivery smoke", &command)
                    .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
                let capture = request_framebuffer_png_at(&config.agent).map_err(device_failure)?;
                delivery_smoke_capture_detail(&capture).map_err(device_failure)?
            }
            DeviceRequest::PrepareBenchmark(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "benchmark prepare",
                    &benchmark_prepare_command(*scenario),
                )
                .map_err(device_failure)?;
                benchmark_scenario_label(*scenario).into()
            }
            DeviceRequest::WarmupBenchmark(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                run_launcher_benchmark(&session, *scenario, true).map_err(device_failure)?;
                "warmed".into()
            }
            DeviceRequest::CaptureBenchmark(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                run_launcher_benchmark(&session, *scenario, false).map_err(device_failure)?;
                remote_read(&session, benchmark_trace_path(false)).ok_or_else(|| {
                    DeviceFailure::OperationFailed("benchmark trace is missing".into())
                })?
            }
            DeviceRequest::RestoreBenchmark => {
                let session = connect(10).map_err(device_failure)?;
                launcher_restart(
                    &session,
                    &LauncherRestartOptions {
                        clear_env: true,
                        ..LauncherRestartOptions::default()
                    },
                )
                .map_err(device_failure)?;
                exec_checked(&session, "benchmark restore", &benchmark_restore_command())
                    .map_err(device_failure)?;
                "restored".into()
            }
            DeviceRequest::SnapshotBenchmarkData(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark suspend",
                    &acknowledged_main_command("mister_magik_suspend"),
                )
                .map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark snapshot",
                    &cold_benchmark_snapshot_command(*scenario),
                )
                .map_err(device_failure)?;
                cold_benchmark_scenario_label(*scenario).into()
            }
            DeviceRequest::EstablishBenchmarkFixture(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark fixture",
                    &cold_benchmark_fixture_command(*scenario),
                )
                .map_err(device_failure)?;
                "fixture-ready".into()
            }
            DeviceRequest::ExecuteColdBenchmark(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark execute",
                    &cold_benchmark_execute_command(*scenario),
                )
                .map_err(device_failure)?;
                "executed".into()
            }
            DeviceRequest::CollectBenchmarkEvents(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                remote_read(&session, cold_benchmark_events_path(*scenario)).ok_or_else(|| {
                    DeviceFailure::OperationFailed("cold benchmark events are missing".into())
                })?
            }
            DeviceRequest::RestoreBenchmarkData(scenario) => {
                let session = connect(10).map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark restore",
                    &cold_benchmark_restore_command(*scenario),
                )
                .map_err(device_failure)?;
                exec_checked(
                    &session,
                    "cold benchmark resume",
                    &acknowledged_main_command("mister_magik_resume"),
                )
                .map_err(device_failure)?;
                "restored".into()
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
                serde_json::to_string(&facts).map_err(device_failure)?
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
                "temporary-state=clear".into()
            }
            DeviceRequest::CaptureFramebuffer => {
                capture_buffer_at(&config.agent, &[]).map_err(device_failure)?;
                "captured".into()
            }
        };
        Ok(DeviceResponse {
            operation: request.label(),
            detail,
        })
    }
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
        "run" => {
            let stream = args.first().map(|s| s.as_str()) == Some("--stream");
            if stream {
                args.remove(0);
            }
            let command = args.first().ok_or("run needs a command")?;
            validate_remote_run_command(command)?;
            let sess = connect(10)?;
            if stream {
                stream_command(&sess, command)?;
            } else {
                let out = exec(&sess, command, true)?;
                print!("{}", out.stdout);
                if !out.stderr.trim().is_empty() {
                    eprint!("[stderr] {}", out.stderr);
                }
                std::process::exit(out.rc);
            }
        }
        "put" => {
            if args.len() < 2 {
                return Err("put needs <local> <remote>".into());
            }
            let sess = connect(10)?;
            put(&sess, Path::new(&args[0]), &args[1])?;
            println!("put {} -> {}", args[0], args[1]);
        }
        "put-dir" => {
            if args.len() < 2 {
                return Err("put-dir needs <local-dir> <remote-dir>".into());
            }
            let sess = connect(10)?;
            let count = put_dir(&sess, Path::new(&args[0]), &args[1])?;
            println!("put-dir {} -> {} files={count}", args[0], args[1]);
        }
        "deploy-magik-bin" => {
            if args.is_empty() {
                return Err("deploy-magik-bin needs <local> [remote]".into());
            }
            let remote = args
                .get(1)
                .cloned()
                .or_else(|| std::env::var("MISTER_MAGIK_BIN").ok())
                .unwrap_or_else(|| "/media/fat/mister-magik/mister-magik-fb".to_string());
            let sess = connect(10)?;
            deploy_magik_bin(&sess, Path::new(&args[0]), &remote)?;
        }
        "platform-deploy" => {
            let stage = args.first().ok_or("platform-deploy needs STAGE_DIR")?;
            let transaction = PlatformDeployTransaction::validate(Path::new(stage))?;
            let sess = connect(10)?;
            transaction.run(&sess)?;
        }
        "platform-rollback" => {
            let sess = connect(10)?;
            exec_checked(&sess, "platform rollback", &platform_rollback_script())?;
        }
        "platform-commit" => {
            let sess = connect(10)?;
            exec_checked(&sess, "platform commit", &platform_cleanup_script())?;
        }
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

fn usage() {
    println!(
        "usage: mister --capture-buffer\n       mister <status|arming-status|mode|scene|display-mode|display-matrix|crt|ini-edit|core-list|catalog|media-check|media-download|agent|reboot-wait|doctor|mame-metadata-build> ...\n       mode <status|dev|public|stock>\n       scene <launcher|controller_test|tear_pattern|video_playback|crt_trial> [seconds]\n       display-mode MODE --attended [--keep]\n         MODE: auto|hdmi-1280x720p60|hdmi-1366x768p60|hdmi-1920x1080p60\n               hdmi-1920x1200p60|hdmi-2048x1536p60|hdmi-2560x1440p60\n               crt-240p60|crt-288p50|crt-480p60|crt-576p50\n       display-matrix --attended --out DIRECTORY\n       crt qualify --attended [--out DIRECTORY]\n       crt qualify --restore\n       ini-edit menu <OUTPUT> [--dry-run]\n       OUTPUT: hdmi|auto|crt-240p60|crt-288p50|crt-480p60|crt-576p50\n               1280x720p60|1024x768p60|720x480p60|720x576p50|1280x1024p60\n               800x600p60|640x480p60|1280x720p50|1920x1080p60|1920x1080p50\n               1366x768p60|1024x600p60|1920x1440p60|2048x1536p60\n       2560x1440p60: Mister does not support 1440p\n       ini-edit stock-boot [--dry-run]\n       mame-metadata-build --out <sqlite> [--listxml <xml>|--mame <bin>|--machine-sqlite <sqlite>]\n       operator commands are typed and bounded; direct-reset-no-sync remains experimental and requires a volatile session token"
    );
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
        framebuffer: Some((320, 240)),
    },
    DisplayMatrixMode {
        id: "crt-288p50",
        output: Some((640, 288)),
        framebuffer: Some((384, 288)),
    },
    DisplayMatrixMode {
        id: "crt-480p60",
        output: Some((640, 480)),
        framebuffer: Some((640, 480)),
    },
    DisplayMatrixMode {
        id: "crt-576p50",
        output: Some((640, 576)),
        framebuffer: Some((640, 480)),
    },
];

static DISPLAY_MATRIX_INTERRUPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct DisplayMatrixEvidence {
    usb_video: bool,
    screensaver_wait_secs: Option<u64>,
}

extern "C" fn display_matrix_interrupt_handler(_: libc::c_int) {
    DISPLAY_MATRIX_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
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
    if readiness.frames_after <= readiness.frames_before {
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
    Ok(DisplayMatrixReadiness {
        output,
        framebuffer,
        frames_before: before,
        frames_after: after,
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
    "set -eu; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then root=/media/fat/mister-magik-dev; else root=/media/fat/mister-magik; fi; report=$(\"$root/mister-magik-fb\" latch-readiness-report --json); printf '%s\\n' \"$report\" | grep -Eq '\"state\"[[:space:]]*:[[:space:]]*\"ready\"'; plan=$(grep '^display-plan:' /tmp/mister-magik-slint.log | tail -n 1); before=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$before\"; after=$before; attempts=0; while test \"$after\" -le \"$before\" && test \"$attempts\" -lt 10; do sleep 1; after=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$after\"; attempts=$((attempts+1)); done; test \"$after\" -gt \"$before\"; printf 'plan\\t%s\\nframes\\t%s\\t%s\\nreadiness\\t%s\\n' \"$plan\" \"$before\" \"$after\" \"$report\"".to_string()
}

fn delivery_health_command(layout: &str) -> Result<String> {
    let (main, directory) = match layout {
        "dev" => ("MiSTer_MagiKDev", "/media/fat/mister-magik-dev"),
        "public" => ("MiSTer_MagiK", "/media/fat/mister-magik"),
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok(format!(
        "set -eu; pidof {main} >/dev/null; pidof mister-magik-fb >/dev/null; grep -q '^mister_magik_scanout_slots ' /proc/modules; test -c /dev/mister-magik-scanout-slots; report=$({directory}/mister-magik-fb latch-readiness-report); printf '%s\\n' \"$report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready'; test ! -e {directory}/launcher.env; test ! -e {directory}/rebuild-on-next-boot; test ! -e /tmp/mister-magik/fs-fault-launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /tmp/mister-magik/fs-fault.json"
    ))
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

fn delivery_smoke_command(layout: &str, expected_sha256: &str) -> Result<String> {
    let (main, directory) = match layout {
        "dev" => ("MiSTer_MagiKDev", "/media/fat/mister-magik-dev"),
        "public" => ("MiSTer_MagiK", "/media/fat/mister-magik"),
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok(format!(
        "set -eu; test \"$(sha256sum {directory}/mister-magik-fb | awk '{{print $1}}')\" = '{expected_sha256}'; pidof {main} >/dev/null; pidof mister-magik-fb >/dev/null; {}; test -n \"$pid_before\"; test \"$pid_before\" = \"$pid_after\"; test -n \"$sequence_before\"; test -n \"$sequence_after\"; test \"$sequence_after\" -gt \"$sequence_before\"; grep -q '^mister_magik_scanout_slots ' /proc/modules; test -c /dev/mister-magik-scanout-slots; report=$({directory}/mister-magik-fb latch-readiness-report); printf '%s\\n' \"$report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready'; grep -Eq '\"scene\"[[:space:]]*:[[:space:]]*\"launcher\"' \"$status\"; grep -Eq '\"screen\"[[:space:]]*:[[:space:]]*\"(home|arcade|settings|systems)\"' \"$status\"; grep -Eq '\"input_enabled\"[[:space:]]*:[[:space:]]*true' \"$status\"; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; test ! -e /media/fat/mister-magik/launcher.env; test ! -e /media/fat/mister-magik-dev/launcher.env; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot; test ! -e /tmp/mister-magik/fs-fault-launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /tmp/mister-magik/fs-fault.json",
        launcher_heartbeat_sample_command()
    ))
}

fn launcher_heartbeat_sample_command() -> &'static str {
    "status=/tmp/mister-magik/status.json; pid_before=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_before=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sleep 2; pid_after=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_after=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\")"
}

fn benchmark_scenario_label(scenario: BenchmarkScenario) -> &'static str {
    match scenario {
        BenchmarkScenario::LauncherVelocity => "launcher-velocity",
        BenchmarkScenario::FramebufferVelocity => "framebuffer-velocity",
    }
}

fn cold_benchmark_scenario_label(scenario: ColdBenchmarkScenario) -> &'static str {
    match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => "catalog-lifecycle",
        ColdBenchmarkScenario::PreviewColdStart => "preview-cold-start",
        ColdBenchmarkScenario::LibraryPersistence => "library-persistence",
    }
}

fn cold_benchmark_events_path(scenario: ColdBenchmarkScenario) -> &'static str {
    match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => {
            "/tmp/mister-magik/agent-catalog-lifecycle.jsonl"
        }
        ColdBenchmarkScenario::PreviewColdStart => {
            "/tmp/mister-magik/agent-preview-cold-start.jsonl"
        }
        ColdBenchmarkScenario::LibraryPersistence => {
            "/tmp/mister-magik/agent-library-persistence.jsonl"
        }
    }
}

fn cold_benchmark_snapshot_command(_scenario: ColdBenchmarkScenario) -> String {
    format!(
        "set -eu; {}; root=/media/fat/mister-magik-dev; snap=/tmp/mister-magik/agent-benchmark-data; rm -rf \"$snap\"; mkdir -p \"$snap\"; for name in catalog-v3 library.sqlite3 arcade-bootstrap.nav.lz4b; do if test -e \"$root/$name\"; then cp -a \"$root/$name\" \"$snap/$name\"; touch \"$snap/$name.present\"; fi; done",
        platform_safety_script()
    )
}

fn cold_benchmark_fixture_command(scenario: ColdBenchmarkScenario) -> String {
    let mutation = match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => "rm -rf \"$root/catalog-v3\"",
        ColdBenchmarkScenario::PreviewColdStart => {
            "rm -f /tmp/mister-magik/preview-* /tmp/mister-magik-slint.log"
        }
        ColdBenchmarkScenario::LibraryPersistence => "rm -f \"$root/library.sqlite3\"",
    };
    format!(
        "set -eu; {}; root=/media/fat/mister-magik-dev; test -d /tmp/mister-magik/agent-benchmark-data; {mutation}; rm -f {}",
        platform_safety_script(),
        cold_benchmark_events_path(scenario)
    )
}

fn cold_benchmark_execute_command(scenario: ColdBenchmarkScenario) -> String {
    let (command, event) = match scenario {
        ColdBenchmarkScenario::CatalogLifecycle => (
            "/media/fat/mister-magik-dev/mister-magik-fb library-refresh",
            "catalog_lifecycle_complete",
        ),
        ColdBenchmarkScenario::PreviewColdStart => (
            "/media/fat/mister-magik-dev/mister-magik-fb preview-index-refresh-bench agent-cold",
            "preview_cold_start_complete",
        ),
        ColdBenchmarkScenario::LibraryPersistence => (
            "/media/fat/mister-magik-dev/mister-magik-fb library-refresh",
            "library_persistence_complete",
        ),
    };
    format!(
        "set -eu; start=$(date +%s); {command} >/tmp/mister-magik/agent-cold-benchmark.out 2>&1; end=$(date +%s); elapsed_ms=$(((end-start)*1000)); printf '{{\"event\":\"{event}\",\"elapsed_ms\":%s,\"status\":\"ok\"}}\\n' \"$elapsed_ms\" >{}",
        cold_benchmark_events_path(scenario)
    )
}

fn cold_benchmark_restore_command(scenario: ColdBenchmarkScenario) -> String {
    format!(
        "set -eu; root=/media/fat/mister-magik-dev; snap=/tmp/mister-magik/agent-benchmark-data; test -d \"$snap\"; rm -rf \"$root/catalog-v3\"; rm -f \"$root/library.sqlite3\" \"$root/arcade-bootstrap.nav.lz4b\"; for name in catalog-v3 library.sqlite3 arcade-bootstrap.nav.lz4b; do if test -e \"$snap/$name.present\"; then mv \"$snap/$name\" \"$root/$name\"; fi; done; rm -rf \"$snap\"; rm -f {} /tmp/mister-magik/agent-cold-benchmark.out; {}",
        cold_benchmark_events_path(scenario),
        platform_safety_script()
    )
}

const RELEASE_TOKEN: &str = "/tmp/mister-magik/release-qualification-session";
const RELEASE_SNAPSHOT: &str = "/tmp/mister-magik/release-qualification-snapshot";

fn release_arming_cleanup_command() -> &'static str {
    "rm -f /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot"
}

fn release_begin_command() -> String {
    format!(
        "set -eu; {}; {} snap={RELEASE_SNAPSHOT}; rm -rf \"$snap\"; mkdir -p \"$snap\"; if test -e /media/fat/MiSTer.ini; then cp -a /media/fat/MiSTer.ini \"$snap/MiSTer.ini\"; fi; printf '%s\\n' attended-non-network-recovery-confirmed >{RELEASE_TOKEN}; test -s {RELEASE_TOKEN}",
        release_arming_cleanup_command(),
        platform_safety_script()
    )
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
    format!(
        "set -eu; test \"$(cat {RELEASE_TOKEN})\" = attended-non-network-recovery-confirmed; test -p /dev/MiSTer_cmd; {}; {}",
        release_arming_cleanup_command(),
        platform_safety_script()
    )
}

fn release_restore_command() -> String {
    format!(
        "set -eu; snap={RELEASE_SNAPSHOT}; {}; if test -s \"$snap/MiSTer.ini\"; then cp -a \"$snap/MiSTer.ini\" /media/fat/MiSTer.ini; fi; rm -f {RELEASE_TOKEN}; rm -rf \"$snap\"; {} test ! -e {RELEASE_TOKEN}",
        release_arming_cleanup_command(),
        platform_safety_script()
    )
}

fn diagnostic_facts_command() -> String {
    format!(
        "set -eu; main=false; launcher=false; agent=false; credentials=false; firmware=false; unstable=false; temporary=false; launcher_heartbeat_advancing=false; {{ pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }} && main=true; pidof mister-magik-fb >/dev/null 2>&1 && launcher=true; pidof mister-magik-agent >/dev/null 2>&1 && agent=true; test -s /media/fat/mister-magik-dev/agent.token && credentials=true; {{ grep -q '^mister_magik_scanout_slots ' /proc/modules 2>/dev/null && test -c /dev/mister-magik-scanout-slots; }} && firmware=true; {}; if test -n \"$pid_before\" && test \"$pid_before\" = \"$pid_after\" && test -n \"$sequence_before\" && test -n \"$sequence_after\" && test \"$sequence_after\" -gt \"$sequence_before\"; then launcher_heartbeat_advancing=true; fi; test -e /tmp/mister-magik/reboot-unstable && unstable=true; arming=0; for path in /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; do test ! -e \"$path\" || arming=$((arming + 1)); done; for path in /tmp/mister-magik/agent-benchmark.tsv /tmp/mister-magik/agent-benchmark-warmup.tsv /tmp/mister-magik/agent-cold-benchmark.out /tmp/mister-magik/stale-launcher-return-state.json; do test ! -e \"$path\" || temporary=true; done; printf '{{\"main_running\":%s,\"launcher_running\":%s,\"agent_running\":%s,\"credentials_ready\":%s,\"firmware_compatible\":%s,\"reboot_unstable\":%s,\"arming_files\":%s,\"temporary_state\":%s,\"launcher_heartbeat_advancing\":%s}}\\n' \"$main\" \"$launcher\" \"$agent\" \"$credentials\" \"$firmware\" \"$unstable\" \"$arming\" \"$temporary\" \"$launcher_heartbeat_advancing\"",
        launcher_heartbeat_sample_command()
    )
}

fn safe_repair_command() -> String {
    format!(
        "set -eu; rm -f /tmp/mister-magik/agent-benchmark.tsv /tmp/mister-magik/agent-benchmark-warmup.tsv /tmp/mister-magik/agent-cold-benchmark.out /tmp/mister-magik/stale-launcher-return-state.json; {}",
        platform_safety_script()
    )
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
    exec_checked(
        &session,
        "mode arming cleanup",
        &format!(
            "set -eu; {}; {}",
            release_arming_cleanup_command(),
            platform_safety_script()
        ),
    )?;
    issue_reboot(&session, RebootMode::Supervised)?;
    drop(session);
    if !wait_down(40.0) || wait_up(120.0)? != 0 {
        return Err("mode switch did not complete its bounded reboot transition".into());
    }
    Ok(())
}

fn installed_platform_verify_command(layout: Layout) -> String {
    let (root, main) = match layout {
        Layout::Development => ("/media/fat/mister-magik-dev", "/media/fat/MiSTer_MagiKDev"),
        Layout::Public => ("/media/fat/mister-magik", "/media/fat/MiSTer_MagiK"),
    };
    format!(
        "set -eu; root={root}; manifest=$root/platform-v2.manifest; test -s \"$manifest\"; test -x {main}; test -x \"$root/mister-magik-fb\"; test -x \"$root/mister-magik-manager\"; test -r \"$root/mister_magik_scanout_slots.ko\"; test -r \"$root/fpga/menu-magik-vblank-latch.rbf\"; grep -qx 'format=mister-magik-platform-v2' \"$manifest\"; get() {{ sed -n \"s/^$1=//p\" \"$manifest\"; }}; test \"$(sha256sum {main} | awk '{{print $1}}')\" = \"$(get main_sha256)\"; test \"$(sha256sum \"$root/mister-magik-fb\" | awk '{{print $1}}')\" = \"$(get gui_sha256)\"; test \"$(sha256sum \"$root/mister-magik-manager\" | awk '{{print $1}}')\" = \"$(get manager_sha256)\"; test \"$(sha256sum \"$root/mister_magik_scanout_slots.ko\" | awk '{{print $1}}')\" = \"$(get scanout_module_sha256)\"; test \"$(sha256sum \"$root/fpga/menu-magik-vblank-latch.rbf\" | awk '{{print $1}}')\" = \"$(get latch_rbf_sha256)\""
    )
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
            "sed -n '/^crt_trial_status_v[23] /p' /tmp/mister-magik-crt_trial.log | tail -n 1",
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
        "sed -n '/^crt_trial_status_v[23] /p' /tmp/mister-magik-crt_trial.log | tail -n 1",
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
    const MARKERS: [&str; 2] = [
        "crt_trial_status_v2 schema=2 ",
        "crt_trial_status_v3 schema=3 ",
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
    if marker == Some(MARKERS[1]) {
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
    Ok(status)
}

fn benchmark_trace_path(warmup: bool) -> &'static str {
    if warmup {
        "/tmp/mister-magik/agent-benchmark-warmup.tsv"
    } else {
        "/tmp/mister-magik/agent-benchmark.tsv"
    }
}

fn benchmark_prepare_command(_scenario: BenchmarkScenario) -> String {
    format!(
        "set -eu; {}; rm -f {} {}; mkdir -p /tmp/mister-magik",
        platform_safety_script(),
        benchmark_trace_path(true),
        benchmark_trace_path(false)
    )
}

fn benchmark_restore_command() -> String {
    format!(
        "set -eu; rm -f {} {}; {}",
        benchmark_trace_path(true),
        benchmark_trace_path(false),
        platform_safety_script()
    )
}

fn run_launcher_benchmark(
    session: &Session,
    scenario: BenchmarkScenario,
    warmup: bool,
) -> Result<()> {
    let trace = benchmark_trace_path(warmup);
    let seconds = if warmup { "2" } else { "8" };
    let scenario_value = match scenario {
        BenchmarkScenario::LauncherVelocity => "velocity-scroll",
        BenchmarkScenario::FramebufferVelocity => "dirty-band",
    };
    launcher_restart(
        session,
        &LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_LAUNCHER_START_SCREEN".into(), "arcade".into()),
                ("MISTER_LAUNCHER_START_SYSTEM".into(), "arcade".into()),
                (
                    "MISTER_LAUNCHER_BENCH_SCENARIO".into(),
                    scenario_value.into(),
                ),
                ("MISTER_PREVIEW_SCROLL_TRACE_SECS".into(), seconds.into()),
                ("MISTER_PREVIEW_SCROLL_TRACE".into(), trace.into()),
                ("MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE".into(), "1".into()),
            ],
            timeout_secs: 30,
            ..LauncherRestartOptions::default()
        },
    )?;
    exec_checked(
        session,
        "benchmark trace wait",
        &format!(
            "set -eu; elapsed=0; while [ $elapsed -lt 20 ]; do test -s {trace} && exit 0; sleep 1; elapsed=$((elapsed + 1)); done; exit 1"
        ),
    )?;
    Ok(())
}

fn action_uses_device(action: &str) -> bool {
    !matches!(
        action,
        "mame-metadata-build" | "profile-summary" | "-h" | "--help"
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

fn delivery_reboot_mode(running_main: &str) -> RebootMode {
    if matches!(running_main.trim(), "MiSTer_MagiKDev" | "MiSTer_MagiK") {
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
    issue_reboot(sess, delivery_reboot_mode(&probe.stdout))
}

fn validate_remote_run_command(command: &str) -> Result<()> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = expand_simple_shell_assignments(&normalized);
    let unquoted = normalized.replace(['\'', '"'], "");
    let direct_arcade = [
        "mister-magik-fb ui arcade",
        "mister-magik-fb' ui arcade",
        "mister-magik-fb\" ui arcade",
        "mister-magic-fb ui arcade",
        "mister-magic-fb' ui arcade",
        "mister-magic-fb\" ui arcade",
    ]
    .iter()
    .any(|needle| normalized.contains(needle) || unquoted.contains(needle));
    if direct_arcade {
        return Err("refusing removed direct arcade scene; benchmark Arcade through the Main-supervised launcher env/restart path".into());
    }
    Ok(())
}

fn expand_simple_shell_assignments(command: &str) -> String {
    let mut expanded = command.to_string();
    for token in command.split_whitespace() {
        let token = token.trim_end_matches(';');
        let Some((name, value)) = token.split_once('=') else {
            continue;
        };
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            continue;
        }
        let value = value.trim_matches(|ch| ch == '\'' || ch == '"');
        expanded = expanded.replace(&format!("${name}"), value);
        expanded = expanded.replace(&format!("${{{name}}}"), value);
    }
    expanded
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

fn deploy_magik_bin(sess: &Session, local: &Path, remote: &str) -> Result<()> {
    let total_t = Instant::now();
    let validate_t = Instant::now();
    let transaction = MagikDeployTransaction::validate(local, remote)?;
    let validate_ms = validate_t.elapsed().as_millis();
    let report = transaction.run_ssh(sess, validate_ms, total_t)?;
    report.print();
    Ok(())
}

const PLATFORM_DEPLOY_FILES: &[(&str, &str)] = &[
    (
        "mister-magik-fb",
        "/media/fat/mister-magik-dev/mister-magik-fb",
    ),
    (
        "mister-magik-manager",
        "/media/fat/mister-magik-dev/mister-magik-manager",
    ),
    ("MiSTer_MagiKDev", "/media/fat/MiSTer_MagiKDev"),
    (
        "mister_magik_scanout_slots.ko",
        "/media/fat/mister-magik-dev/mister_magik_scanout_slots.ko",
    ),
    (
        "mister_magik_scanout_slots.metadata.txt",
        "/media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt",
    ),
    (
        "fpga/menu-magik-vblank-latch.rbf",
        "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf",
    ),
    (
        "fpga/menu-magik-vblank-latch.metadata.txt",
        "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt",
    ),
    ("mame.sqlite3", "/media/fat/mister-magik-dev/mame.sqlite3"),
    (
        "hbmame.sqlite3",
        "/media/fat/mister-magik-dev/hbmame.sqlite3",
    ),
    (
        "game-databases-manifest.json",
        "/media/fat/mister-magik-dev/game-databases-manifest.json",
    ),
    (
        "game-databases-SHA256SUMS",
        "/media/fat/mister-magik-dev/game-databases-SHA256SUMS",
    ),
    (
        "platform-v2.manifest",
        "/media/fat/mister-magik-dev/platform-v2.manifest",
    ),
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlatformDeployTransaction {
    stage: PathBuf,
    files: Vec<PlatformDeployFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlatformDeployFile {
    local: PathBuf,
    remote: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlatformDeployReport {
    changed_files: usize,
    skipped_files: usize,
    transferred_bytes: u64,
}

impl PlatformDeployTransaction {
    fn validate(stage: &Path) -> Result<Self> {
        if !stage.is_dir() {
            return Err(format!("platform stage is missing: {}", stage.display()).into());
        }
        let mut files = Vec::new();
        for (relative, remote) in PLATFORM_DEPLOY_FILES {
            let local = stage.join(relative);
            if !local.is_file() {
                return Err(format!("platform stage is missing {relative}").into());
            }
            files.push(PlatformDeployFile {
                bytes: fs::metadata(&local)?.len(),
                sha256: file_sha256(local.clone())?,
                local,
                remote: (*remote).into(),
            });
        }
        Ok(Self {
            stage: stage.to_path_buf(),
            files,
        })
    }

    fn run(&self, sess: &Session) -> Result<PlatformDeployReport> {
        self.run_with(&SshDeployRemote { sess })
    }

    fn run_with<R: DeployRemote>(&self, remote: &R) -> Result<PlatformDeployReport> {
        let inventory = remote.exec(&self.inventory_command())?;
        if let Some(message) = exec_failure_message("platform inventory", &inventory) {
            return Err(message.into());
        }
        let installed = self.parse_inventory(&inventory.stdout)?;
        let changed = self
            .files
            .iter()
            .zip(installed)
            .filter_map(|(file, installed)| {
                (installed.as_deref() != Some(&file.sha256)).then_some(file)
            })
            .collect::<Vec<_>>();
        let report = PlatformDeployReport {
            changed_files: changed.len(),
            skipped_files: self.files.len().saturating_sub(changed.len()),
            transferred_bytes: changed.iter().map(|file| file.bytes).sum(),
        };
        if changed.is_empty() {
            println!(
                "platform deploy ok stage={} changed_files=0 skipped_files={} transferred_bytes=0",
                self.stage.display(),
                report.skipped_files,
            );
            return Ok(report);
        }

        remote
            .exec("mkdir -p /media/fat/mister-magik-dev/fpga /media/fat/mister-magik-dev/snapshots")
            .and_then(|output| checked_deploy_output("platform prepare", output))?;
        remote.exec(
            "set -e; stamp=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown); snapshot=/media/fat/mister-magik-dev/snapshots/$stamp-agent-deploy; mkdir -p \"$snapshot\"; cp /etc/inittab \"$snapshot/inittab\" 2>/dev/null || true; cp /media/fat/MiSTer.ini \"$snapshot/MiSTer.ini\" 2>/dev/null || true; cp /media/fat/mister-magik-dev/platform-v2.manifest \"$snapshot/platform-v2.manifest\" 2>/dev/null || true",
        ).and_then(|output| checked_deploy_output("platform snapshot", output))?;
        for file in &changed {
            remote.put(&file.local, &format!("{}.upload", file.remote))?;
        }
        let script = self.activation_script(&changed);
        let output = remote.exec(&script)?;
        if let Some(message) = exec_failure_message("platform activation", &output) {
            return Err(message.into());
        }
        println!(
            "platform deploy ok stage={} changed_files={} skipped_files={} transferred_bytes={}",
            self.stage.display(),
            report.changed_files,
            report.skipped_files,
            report.transferred_bytes,
        );
        Ok(report)
    }

    fn inventory_command(&self) -> String {
        let mut command = String::from("set -eu; ");
        for file in &self.files {
            command.push_str(&format!(
                "if test -f {path}; then sha256sum {path}; else printf 'missing  %s\\n' {path}; fi; ",
                path = sh(&file.remote),
            ));
        }
        command
    }

    fn parse_inventory(&self, stdout: &str) -> Result<Vec<Option<String>>> {
        let lines = stdout.lines().collect::<Vec<_>>();
        if lines.len() != self.files.len() {
            return Err(format!(
                "platform inventory returned {} lines for {} files",
                lines.len(),
                self.files.len()
            )
            .into());
        }
        lines
            .into_iter()
            .zip(&self.files)
            .map(|(line, file)| {
                let mut fields = line.split_whitespace();
                let fingerprint = fields.next().unwrap_or_default();
                let path = fields.next().unwrap_or_default();
                if path != file.remote {
                    return Err(format!(
                        "platform inventory path mismatch: expected {} got {}",
                        file.remote, path
                    )
                    .into());
                }
                if fingerprint == "missing" {
                    return Ok(None);
                }
                if fingerprint.len() != 64
                    || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(
                        format!("platform inventory invalid SHA-256 for {}", file.remote).into(),
                    );
                }
                Ok(Some(fingerprint.to_ascii_lowercase()))
            })
            .collect()
    }

    fn activation_script(&self, changed: &[&PlatformDeployFile]) -> String {
        let mut verify = String::new();
        let mut clear_stale = String::new();
        let mut backup = String::new();
        let mut activate = String::new();
        let mut rollback = String::new();
        for file in &self.files {
            clear_stale.push_str(&format!(
                "rm -f {rollback} {missing}; ",
                rollback = sh(&format!("{}.rollback", file.remote)),
                missing = sh(&format!("{}.rollback-missing", file.remote)),
            ));
        }
        for file in changed {
            verify.push_str(&format!(
                "test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; ",
                sh(&format!("{}.upload", file.remote)),
                sh(&file.sha256)
            ));
            backup.push_str(&format!(
                "if [ -e {path} ]; then cp -p {path} {backup}; else : > {missing}; fi; ",
                path = sh(&file.remote),
                backup = sh(&format!("{}.rollback", file.remote)),
                missing = sh(&format!("{}.rollback-missing", file.remote))
            ));
            rollback.push_str(&format!(
                "if [ -e {backup} ]; then mv -f {backup} {path}; elif [ -e {missing} ]; then rm -f {path} {missing}; fi; ",
                path = sh(&file.remote),
                backup = sh(&format!("{}.rollback", file.remote)),
                missing = sh(&format!("{}.rollback-missing", file.remote))
            ));
        }
        for file in changed
            .iter()
            .filter(|file| !file.remote.ends_with("platform-v2.manifest"))
        {
            activate.push_str(&format!(
                "mv -f {} {}; ",
                sh(&format!("{}.upload", file.remote)),
                sh(&file.remote)
            ));
        }
        if let Some(manifest) = changed
            .iter()
            .find(|file| file.remote.ends_with("platform-v2.manifest"))
        {
            activate.push_str(&format!(
                "mv -f {} {}; ",
                sh(&format!("{}.upload", manifest.remote)),
                sh(&manifest.remote)
            ));
        }
        let mut chmod = String::new();
        for file in changed.iter().filter(|file| {
            file.remote.ends_with("/mister-magik-fb")
                || file.remote.ends_with("/mister-magik-manager")
                || file.remote == "/media/fat/MiSTer_MagiKDev"
        }) {
            chmod.push_str(&format!("chmod 755 {}; ", sh(&file.remote)));
        }
        format!(
            "set -eu; rm -f /media/fat/MiSTer.ini.platform-rollback; cp -p /media/fat/MiSTer.ini /media/fat/MiSTer.ini.platform-rollback; {verify} {clear_stale} {backup} rollback() {{ {rollback} mv -f /media/fat/MiSTer.ini.platform-rollback /media/fat/MiSTer.ini 2>/dev/null || true; sync; }}; trap rollback EXIT INT TERM; {activate} {chmod} sync; {safety} trap - EXIT INT TERM; sync",
            safety = platform_safety_script(),
        )
    }
}

fn checked_deploy_output(label: &str, output: ExecOutput) -> Result<ExecOutput> {
    if let Some(message) = exec_failure_message(label, &output) {
        Err(message.into())
    } else {
        Ok(output)
    }
}

fn platform_rollback_script() -> String {
    let mut rollback = String::from("set -eu; ");
    for (_, remote) in PLATFORM_DEPLOY_FILES {
        rollback.push_str(&format!(
            "if [ -e {backup} ]; then mv -f {backup} {path}; elif [ -e {missing} ]; then rm -f {path} {missing}; fi; ",
            path = sh(remote), backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing"))
        ));
    }
    rollback.push_str("mv -f /media/fat/MiSTer.ini.platform-rollback /media/fat/MiSTer.ini 2>/dev/null || true; sync; ");
    rollback.push_str(&platform_safety_script());
    rollback
}

fn platform_snapshot_script() -> String {
    let mut cleanup = String::from("rm -f /media/fat/MiSTer.ini.platform-rollback; ");
    let mut snapshot = String::new();
    for (_, remote) in PLATFORM_DEPLOY_FILES {
        cleanup.push_str(&format!(
            "rm -f {backup} {missing}; ",
            backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing"))
        ));
        snapshot.push_str(&format!(
            "if [ -e {path} ]; then cp -p {path} {backup}; else : > {missing}; fi; ",
            path = sh(remote),
            backup = sh(&format!("{remote}.rollback")),
            missing = sh(&format!("{remote}.rollback-missing"))
        ));
    }
    format!(
        "set -eu; cleanup() {{ {cleanup} }}; cleanup; trap cleanup EXIT INT TERM; cp -p /media/fat/MiSTer.ini /media/fat/MiSTer.ini.platform-rollback; {snapshot} sync; trap - EXIT INT TERM"
    )
}

fn platform_cleanup_script() -> String {
    let mut cleanup = format!("set -eu; {} ", platform_safety_script());
    for (_, remote) in PLATFORM_DEPLOY_FILES {
        cleanup.push_str(&format!(
            "rm -f {} {}; ",
            sh(&format!("{remote}.rollback")),
            sh(&format!("{remote}.rollback-missing"))
        ));
    }
    cleanup.push_str("rm -f /media/fat/MiSTer.ini.platform-rollback; sync; ");
    cleanup
}

fn platform_safety_script() -> String {
    "test ! -e /media/fat/mister-magik/launcher.env; test ! -e /media/fat/mister-magik-dev/launcher.env; test ! -e /tmp/mister-magik/fs-fault-launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /tmp/mister-magik/fs-fault.json; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot;".into()
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
    fn validate(local: &Path, remote: &str) -> Result<Self> {
        if !remote.starts_with('/') || remote.ends_with('/') || remote.contains('\0') {
            return Err(format!("unsupported deploy remote: {remote}").into());
        }
        if remote.split('/').any(|part| part == "." || part == "..") {
            return Err(format!("unsupported deploy remote path component: {remote}").into());
        }
        let remote_dir = remote_parent_dir(remote)?.to_string();
        let local_bytes = fs::metadata(local)?.len();
        Ok(Self {
            local: local.to_path_buf(),
            remote: remote.to_string(),
            upload: format!("{remote}.upload"),
            lock: format!("{remote_dir}/deploy.lock"),
            remote_dir,
            local_bytes,
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
        self.exec_phase(
            remote,
            "swap",
            &format!("mv {} {}", sh(&self.upload), sh(&self.remote)),
        )?;
        Ok(start.elapsed().as_millis())
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
        self.exec_phase(
            remote,
            "cleanup",
            &format!("rm -f {} {}", sh(&self.upload), sh(&self.lock)),
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
            "deploy_magik_bin local={} remote={} local_bytes={} remote_bytes={} total_ms={} prepare_ms={} suspend_ms={} put_ms={} finish_ms={} resume_size_ms={} validate_ms={} upload_ms={} swap_ms={} chmod_size_ms={} resume_ms={} cleanup_ms={}",
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
        "deploy-magik-bin" => {
            agent_deploy_magik_bin(&args[1..])?;
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
        "usage: mister agent <ping|status|logs|timeline|sd-list|diagnostics|framebuffer-capture|framebuffer-capture-raw|framebuffer-capture-lz4|deploy-magik-bin|magik|reboot-wait|boot-profile>\n       logs [--json]\n       timeline [--json]\n       sd-list PATH [--protocol auto|v1|v2] [--show-hidden] [--repeat N] [--json]\n       diagnostics [--out DIR]\n       framebuffer-capture OUT.png [--json OUT.json]\n       framebuffer-capture-raw OUT.raw [--json OUT.json]\n       framebuffer-capture-lz4 OUT.raw [--json OUT.json]\n       deploy-magik-bin LOCAL [REMOTE]\n       magik <status|suspend|resume|restart-launcher>\n       reboot-wait [--timeout SECS] [--raw|--direct-reset|--direct-reset-no-sync]\n       boot-profile [samples] [--timeout SECS] [--probe-timeout-ms MS] [--sleep-ms MS] [--raw|--direct-reset|--direct-reset-no-sync] [--fail-on-timeout]"
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
    if source != source_kind || !matches!(source, "fb0" | "fpga-latched-scanout-slots") {
        return Err(format!("agent framebuffer capture returned invalid source {source:?}").into());
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
    }
    Ok(())
}

fn validate_visible_launcher_capture(capture: &PngCapture) -> Result<()> {
    let result = &capture.result;
    if capture_source_label(result)? != "fpga-latched-scanout-slots" {
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

fn agent_deploy_magik_bin(args: &[String]) -> Result<()> {
    let json_output = args.iter().any(|arg| arg == "--json");
    let positional = args
        .iter()
        .filter(|arg| arg.as_str() != "--json")
        .collect::<Vec<_>>();
    let local = positional
        .first()
        .ok_or("agent deploy-magik-bin needs LOCAL [REMOTE]")?;
    if positional.len() > 2 {
        return Err("usage: mister agent deploy-magik-bin LOCAL [REMOTE] [--json]".into());
    }
    let remote = positional
        .get(1)
        .map(|value| (*value).clone())
        .or_else(|| std::env::var("MISTER_MAGIK_BIN").ok())
        .unwrap_or_else(|| "/media/fat/mister-magik/mister-magik-fb".to_string());
    let total_t = Instant::now();
    let read_t = Instant::now();
    let mut source = fs::File::open(local)?;
    let byte_count = source.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut hash_buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut hash_buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&hash_buffer[..count]);
    }
    let checksum = encode_hex(&hasher.finalize());
    source = fs::File::open(local)?;
    let read_ms = read_t.elapsed().as_millis();
    let encoding = "raw";
    let compression_decision = "streamed-raw";
    let compress_ms = 0;
    let args = json!({
        "remote": &remote,
        "size": byte_count,
        "payload_size": byte_count,
        "checksum": checksum,
        "encoding": encoding,
    });
    let reply = agent_stream_request_reader(
        "deploy_magik_bin_stream",
        args,
        &mut source,
        Duration::from_secs(120),
    )?;
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    let remote_bytes = verify_agent_deploy_result(result, byte_count, &remote, &checksum)?;
    let output = json!({
        "action": "deploy-magik-bin",
        "local": local,
        "remote": remote,
        "encoding": encoding,
        "compression_decision": compression_decision,
        "bytes": byte_count,
        "remote_bytes": remote_bytes,
        "payload_bytes": byte_count,
        "checksum": checksum,
        "total_ms": total_t.elapsed().as_millis() as u64,
        "read_ms": read_ms as u64,
        "compress_ms": compress_ms,
        "request_ms": reply.elapsed_ms,
        "result": result,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", format_agent_deploy_summary(&output));
    }
    Ok(())
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

fn format_agent_deploy_summary(output: &Value) -> String {
    let remote = output.get("remote").and_then(Value::as_str).unwrap_or("?");
    let bytes = output.get("bytes").and_then(Value::as_u64).unwrap_or(0);
    let total_ms = output.get("total_ms").and_then(Value::as_u64).unwrap_or(0);
    let checksum = output
        .get("checksum")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let checksum = checksum.get(..12).unwrap_or(checksum);
    format!(
        "agent deploy-magik-bin ok remote={remote} bytes={bytes} elapsed_ms={total_ms} sha256={checksum}"
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
        Some("stock-boot") => Ok(IniEdit::StockBoot),
        _ => unreachable!("validated ini-edit arguments must parse"),
    }
}

fn validate_ini_edit_args(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("menu") if args.len() == 2 => {
            MenuOutputProfile::parse(&args[1])?;
        }
        Some("stock-boot") if args.len() == 1 => {}
        Some("menu" | "stock-boot") => {
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
            "  main:      visible_owner={} launcher_pid={} osd_suppressed={}",
            main.get("visible_owner")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            main.get("launcher_pid")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into()),
            main.get("osd_suppressed_count")
                .map(Value::to_string)
                .unwrap_or_else(|| "?".into())
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
    fn display_matrix_readiness_requires_geometry_and_advancing_frames() {
        let parsed = parse_display_matrix_readiness(
            "plan\tdisplay-plan: output=640x240 scan=640x240 fb=320x240\nframes\t10\t12\n",
        )
        .unwrap();
        assert_eq!(
            parsed,
            DisplayMatrixReadiness {
                output: (640, 240),
                framebuffer: (320, 240),
                frames_before: 10,
                frames_after: 12,
            }
        );
        assert!(parse_display_matrix_readiness("frames\t10\t12\n").is_err());
    }

    #[test]
    fn display_matrix_geometry_rejects_wrong_framebuffer() {
        let mode = DISPLAY_MATRIX_MODES
            .iter()
            .find(|mode| mode.id == "crt-240p60")
            .copied()
            .unwrap();
        assert!(validate_display_matrix_geometry(mode, (640, 240), (320, 240)).is_ok());
        assert!(validate_display_matrix_geometry(mode, (640, 240), (640, 240)).is_err());
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
    fn formats_compact_agent_deploy_summary() {
        let output = json!({
            "remote": "/media/fat/mister-magik-dev/mister-magik-fb",
            "bytes": 123456,
            "total_ms": 987,
            "checksum": "0123456789abcdef",
            "result": {"verbose": "omitted"},
        });
        assert_eq!(
            format_agent_deploy_summary(&output),
            "agent deploy-magik-bin ok remote=/media/fat/mister-magik-dev/mister-magik-fb bytes=123456 elapsed_ms=987 sha256=0123456789ab"
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
                "main_status": {"visible_owner": "fb0"}
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
    fn remote_run_rejects_removed_direct_arcade_scene() {
        assert!(
            validate_remote_run_command("/media/fat/mister-magik/mister-magik-fb ui arcade 20")
                .is_err()
        );
        assert!(
            validate_remote_run_command("'/media/fat/mister-magik/mister-magik-fb' ui arcade 20")
                .is_err()
        );
        assert!(
            validate_remote_run_command(
                "scene=arcade; /media/fat/mister-magik/mister-magik-fb ui \"$scene\" 20"
            )
            .is_err()
        );
    }

    #[test]
    fn remote_run_allows_launcher_and_restart_paths() {
        assert!(
            validate_remote_run_command("/media/fat/mister-magik/mister-magik-fb ui launcher 0")
                .is_ok()
        );
        assert!(
            validate_remote_run_command(
                "printf 'mister_magik_restart_launcher\\n' > /dev/MiSTer_cmd"
            )
            .is_ok()
        );
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
            delivery_reboot_mode("MiSTer_MagiKDev"),
            RebootMode::Supervised
        );
        assert_eq!(delivery_reboot_mode("MiSTer_MagiK"), RebootMode::Supervised);
        assert_eq!(delivery_reboot_mode("MiSTer"), RebootMode::Raw);
        assert_eq!(delivery_reboot_mode(""), RebootMode::Raw);
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
    fn deploy_transaction_derives_remote_paths_and_local_size() {
        let local = temp_path("deploy-bin");
        fs::write(&local, b"abc").unwrap();

        let tx =
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/mister-magik-fb")
                .unwrap();
        let _ = fs::remove_file(&local);

        assert_eq!(tx.remote_dir, "/media/fat/mister-magik");
        assert_eq!(tx.upload, "/media/fat/mister-magik/mister-magik-fb.upload");
        assert_eq!(tx.lock, "/media/fat/mister-magik/deploy.lock");
        assert_eq!(tx.local_bytes, 3);
        assert_eq!(
            tx.chmod_size_verify_command(),
            "chmod +x '/media/fat/mister-magik/mister-magik-fb' && wc -c '/media/fat/mister-magik/mister-magik-fb'"
        );
    }

    #[test]
    fn deploy_transaction_rejects_invalid_remote_paths() {
        let local = temp_path("deploy-invalid-bin");
        fs::write(&local, b"abc").unwrap();

        assert!(MagikDeployTransaction::validate(&local, "relative/path").is_err());
        assert!(MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/").is_err());
        assert!(
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/../MiSTer").is_err()
        );

        let _ = fs::remove_file(&local);
    }

    #[test]
    fn deploy_transaction_runs_bounded_phases_in_order() {
        let local = temp_path("deploy-scripted-success");
        fs::write(&local, b"abc").unwrap();
        let tx =
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/mister-magik-fb")
                .unwrap();
        let remote = scripted_deploy_remote(3);

        let report = tx.run_with(&remote, 0, Instant::now()).unwrap();
        let events = remote.events();

        assert_eq!(report.remote_bytes, 3);
        assert!(events[0].contains("mkdir -p"));
        assert!(events[1].contains("mister_magik_suspend"));
        assert!(events[2].starts_with("put "));
        assert!(events[3].starts_with("mv "));
        assert!(events[4].contains("wc -c"));
        assert!(events[5].starts_with("rm -f "));
        assert!(events[6].contains("mister_magik_resume"));
        let _ = fs::remove_file(local);
    }

    #[test]
    fn deploy_transaction_cleans_and_resumes_after_upload_failure() {
        let local = temp_path("deploy-scripted-upload-failure");
        fs::write(&local, b"abc").unwrap();
        let tx =
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/mister-magik-fb")
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
    }

    #[test]
    fn deploy_transaction_cleans_partial_prepare_failure() {
        let local = temp_path("deploy-scripted-prepare-failure");
        fs::write(&local, b"abc").unwrap();
        let tx =
            MagikDeployTransaction::validate(&local, "/media/fat/mister-magik/mister-magik-fb")
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
    fn agent_deploy_result_verifies_remote_and_size() {
        let result = json!({
            "remote": "/media/fat/mister-magik/mister-magik-fb",
            "remote_bytes": 42,
            "checksum_algorithm": "sha256",
            "checksum": "abc",
            "published": true,
            "rolled_back": false
        });

        assert_eq!(
            verify_agent_deploy_result(
                &result,
                42,
                "/media/fat/mister-magik/mister-magik-fb",
                "abc"
            )
            .unwrap(),
            42
        );
        assert!(
            verify_agent_deploy_result(
                &result,
                43,
                "/media/fat/mister-magik/mister-magik-fb",
                "abc"
            )
            .is_err()
        );
        assert!(verify_agent_deploy_result(&result, 42, "/tmp/other", "abc").is_err());
        assert!(
            verify_agent_deploy_result(
                &result,
                42,
                "/media/fat/mister-magik/mister-magik-fb",
                "wrong"
            )
            .is_err()
        );
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
    fn launcher_smoke_rejects_fallback_and_blank_authoritative_capture() {
        let mut result = json!({
            "schema": "mister-magik-framebuffer-capture-v2",
            "source": "fb0",
            "capture_source": {"kind": "fb0"},
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
            .find("platform-v2.manifest.upload' '/media/fat/mister-magik-dev/platform-v2.manifest'")
            .unwrap();
        assert!(manifest > gui);
        assert!(script.contains("trap rollback EXIT INT TERM"));
        assert!(script.contains("test ! -e /tmp/mister-magik/fs-fault-session"));
        assert!(script.contains("test ! -e /tmp/mister-magik/fs-fault-launcher.env"));
        assert!(script.contains("test ! -e /tmp/mister-magik/fs-fault.json"));
        let stale_rollback = script
            .find("rm -f '/media/fat/MiSTer_MagiKDev.rollback'")
            .unwrap();
        let fresh_backup = script
            .find("cp -p '/media/fat/MiSTer_MagiKDev' '/media/fat/MiSTer_MagiKDev.rollback'")
            .unwrap();
        assert!(stale_rollback < fresh_backup);
        let cleanup = platform_cleanup_script();
        assert!(
            cleanup.find("fs-fault.json").unwrap()
                < cleanup.find("MiSTer_MagiKDev.rollback").unwrap()
        );
        assert!(platform_rollback_script().contains("MiSTer.ini.platform-rollback"));
        let snapshot = platform_snapshot_script();
        assert!(snapshot.contains("trap cleanup EXIT INT TERM"));
        assert!(snapshot.contains("MiSTer_MagiKDev.rollback"));
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
        let manifest = "/media/fat/mister-magik-dev/platform-v2.manifest";
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
    fn delivery_smoke_owns_every_fixed_safety_check() {
        let command = delivery_smoke_command("dev", &"a".repeat(64)).unwrap();
        for required in [
            "sha256sum",
            "pidof MiSTer_MagiKDev",
            "pidof mister-magik-fb",
            "mister_magik_scanout_slots",
            "latch-readiness-report",
            "\"scene\"",
            "\"screen\"",
            "\"input_enabled\"",
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
        assert!(validate_delivery_remote("/tmp/not-owned").is_err());
    }

    #[test]
    fn diagnostic_facts_sample_launcher_heartbeat() {
        let command = diagnostic_facts_command();
        assert!(command.contains("status_sequence"));
        assert!(command.contains("launcher_heartbeat_advancing"));
        assert!(command.contains("pid_before"));
        assert!(command.contains("pid_after"));
    }

    #[test]
    fn release_recovery_requires_volatile_token_and_clears_every_arming_path() {
        assert!(!release_begin_command().contains(";;"));
        let catalog = release_catalog_command();
        assert!(catalog.contains("pidof MiSTer_MagiKDev"));
        assert!(catalog.contains("root=/media/fat/mister-magik-dev"));
        let recovery = release_recovery_command();
        assert!(recovery.contains(RELEASE_TOKEN));
        assert!(recovery.contains("attended-non-network-recovery-confirmed"));
        let restore = release_restore_command();
        assert!(!restore.contains(";;"));
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
        assert!(!repair.contains("rm -f /media/fat/mister-magik/launcher.env"));
        assert!(!repair.contains("rm -f /media/fat/mister-magik-dev/launcher.env"));
        assert!(!repair.contains("rebuild-on-next-boot; rm"));
    }

    #[test]
    fn typed_operator_commands_own_platform_and_scene_safety() {
        for layout in [Layout::Development, Layout::Public] {
            let verify = installed_platform_verify_command(layout);
            assert!(verify.contains("platform-v2.manifest"));
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
