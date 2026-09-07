// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::transport::{DeviceFailure, Layout};
use mister_magik_platform_manifest_contract as platform_manifest_contract;
#[cfg(test)]
use quick_xml::Reader;
#[cfg(test)]
use quick_xml::events::{BytesStart, Event};
#[cfg(test)]
use rusqlite::Connection;
#[cfg(test)]
use rusqlite::params;
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
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod agent_client;
mod crt_qualification;
mod discovery;
mod framebuffer_views;
mod installed_layout;
mod platform_deploy;
mod remote;

use agent_client::{
    AGENT_PORT, AgentEndpoint, agent_request, agent_request_at, agent_request_with_liveness,
    bootstrap_agent_with,
};
use platform_deploy::*;
use remote::{
    ConnectionConfig, ExecOutput, acknowledged_main_command, connect, connect_with,
    create_dir_command, exec, exec_failure_message, host, host_wait_diagnostics_with,
    launcher_restart_command, port_open_with, put, put_bytes, remove_files_command,
    shell_quote as sh, tcp_probe_label_port_with,
};

#[cfg(test)]
const DEFAULT_FB_W: usize = 1920;
#[cfg(test)]
const DEFAULT_FB_H: usize = 1080;
#[cfg(test)]
const DEFAULT_FB_BPP: usize = 32;
const RAW_REBOOT_REMOTE_CMD: &str = "nohup /sbin/reboot >/dev/null 2>&1 & echo raw";
#[cfg(test)]
static DEFAULT_REMOTE_LIBRARY_DB: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Public, "library.sqlite3").expect("static installed path")
});
static DEFAULT_LAUNCHER_ENV_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Public, "launcher.env").expect("static installed path")
});
static DEVELOPMENT_LAUNCHER_ENV_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Development, "launcher.env").expect("static installed path")
});

static DEVELOPMENT_AGENT_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Development, "mister-magik-agent")
        .expect("static installed path")
});
const MAIN_STATUS_REMOTE: &str = "/tmp/mister-magik/main-status.json";
const SLINT_STATUS_REMOTE: &str = "/tmp/mister-magik/status.json";

const DEVELOPMENT_GUI_REMOTE: &str = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.gui;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn configured_remote_path(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

fn development_gui_command(subcommand: &str) -> String {
    format!("{DEVELOPMENT_GUI_REMOTE} {subcommand}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RebootMode {
    Supervised,
    Raw,
}

impl RebootMode {
    fn label(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Raw => "raw",
        }
    }
}

#[derive(Clone, Debug)]
struct NativeDeviceConfig {
    connection: ConnectionConfig,
    device_id: String,
    agent: Option<AgentEndpoint>,
}

impl NativeDeviceConfig {
    fn new(connection: ConnectionConfig, device_id: String) -> Self {
        Self {
            connection,
            device_id,
            agent: None,
        }
    }

    fn agent(&self) -> Result<&AgentEndpoint> {
        self.agent
            .as_ref()
            .ok_or_else(|| "device agent was not prepared for this operation".into())
    }
}

#[derive(Default)]
pub struct NativeDevice {
    config: Option<NativeDeviceConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveRuntime {
    executable_path: Option<String>,
    launcher_state: Option<String>,
}

impl ActiveRuntime {
    pub(crate) fn new(executable_path: Option<&str>, launcher_state: Option<&str>) -> Self {
        Self {
            executable_path: executable_path.map(str::to_owned),
            launcher_state: launcher_state.map(str::to_owned),
        }
    }

    pub(crate) fn is_development_launcher(&self) -> bool {
        self.executable_path.as_deref() == Some(installed_layout::paths(Layout::Development).main)
            && self.launcher_state.as_deref() == Some("LauncherActive")
    }

    pub(crate) fn description(&self) -> String {
        format!(
            "executable_path={} launcher_state={}",
            self.executable_path.as_deref().unwrap_or("unknown"),
            self.launcher_state.as_deref().unwrap_or("unknown")
        )
    }
}

struct DeviceProcessLock {
    file: fs::File,
}

impl DeviceProcessLock {
    fn acquire(device_id: &str) -> std::result::Result<Self, DeviceFailure> {
        let directory = discovery::state_dir()
            .map_err(device_failure)?
            .join("locks");
        Self::acquire_at(&directory, device_id)
    }

    fn acquire_at(directory: &Path, device_id: &str) -> std::result::Result<Self, DeviceFailure> {
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
        fs::create_dir_all(directory).map_err(device_failure)?;
        let path = directory.join(format!("device-{safe_id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                DeviceFailure::OperationFailed(format!(
                    "cannot open device lock {}: {error}",
                    path.display()
                ))
            })?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(DeviceFailure::Busy(
                "another process is mutating this device".into(),
            ));
        }
        Ok(Self { file })
    }
}

impl Drop for DeviceProcessLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone, Copy)]
struct DeviceAccess {
    agent: bool,
    mutation: bool,
}

impl DeviceAccess {
    const SSH_READ: Self = Self {
        agent: false,
        mutation: false,
    };
    const SSH_MUTATION: Self = Self {
        agent: false,
        mutation: true,
    };
    const AGENT_READ: Self = Self {
        agent: true,
        mutation: false,
    };
    const AGENT_MUTATION: Self = Self {
        agent: true,
        mutation: true,
    };
}

struct PreparedDevice {
    config: NativeDeviceConfig,
    _lock: Option<DeviceProcessLock>,
}

impl NativeDevice {
    fn prepare(
        &mut self,
        access: DeviceAccess,
    ) -> std::result::Result<PreparedDevice, DeviceFailure> {
        if self.config.is_none() {
            let device = discovery::resolve().map_err(device_failure)?;
            let connection = ConnectionConfig::for_resolved_host(device.address.to_string());
            self.config = Some(NativeDeviceConfig::new(connection, device.id));
        }
        let config = self.config.as_ref().ok_or_else(|| {
            DeviceFailure::OperationFailed("device configuration is unavailable".into())
        })?;
        let needs_bootstrap = access.agent && config.agent.is_none();
        let mut lock = if access.mutation || needs_bootstrap {
            Some(DeviceProcessLock::acquire(&config.device_id)?)
        } else {
            None
        };
        if needs_bootstrap {
            let explicit_token = env::var("MISTER_AGENT_TOKEN")
                .ok()
                .filter(|token| !token.trim().is_empty());
            let token = bootstrap_agent_with(
                &config.connection,
                &config.device_id,
                explicit_token.as_deref(),
            )
            .map_err(|error| {
                DeviceFailure::OperationFailed(format!("agent bootstrap failed: {error}"))
            })?;
            self.config
                .as_mut()
                .expect("device configuration was just resolved")
                .agent = Some(AgentEndpoint::new(config.connection.host(), token));
        }
        if !access.mutation {
            lock.take();
        }
        Ok(PreparedDevice {
            config: self.config.clone().ok_or_else(|| {
                DeviceFailure::OperationFailed("device configuration is unavailable".into())
            })?,
            _lock: lock,
        })
    }

    pub(crate) fn discover(&mut self) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        Ok(())
    }

    pub(crate) fn run_operator(
        &mut self,
        command: &crate::commands::device::DeviceCommand,
    ) -> std::result::Result<(), DeviceFailure> {
        use crate::commands::device::{
            CaptureCommand, CrtCommand, DeviceCommand, DeviceFpgaCommand,
        };
        let agent = !matches!(command, DeviceCommand::ArmingStatus);
        let mutation = command.is_mutation();
        let access = match (agent, mutation) {
            (false, false) => DeviceAccess::SSH_READ,
            (false, true) => DeviceAccess::SSH_MUTATION,
            (true, false) => DeviceAccess::AGENT_READ,
            (true, true) => DeviceAccess::AGENT_MUTATION,
        };
        let prepared = self.prepare(access)?;
        install_prepared_device_environment(&prepared.config);
        let result = (|| -> Result<()> {
            match command {
                DeviceCommand::ArmingStatus => arming_status(),
                DeviceCommand::Crt { command } => match command {
                    CrtCommand::Qualify(args) => {
                        let mut values = device_strings(["qualify", "--attended"]);
                        if let Some(out) = &args.out {
                            values.extend(["--out".into(), out.to_string_lossy().into_owned()]);
                        }
                        crt_qualification::run(&values)
                    }
                    CrtCommand::Probe(args) => crt_qualification::run(&device_strings([
                        "probe",
                        "--attended",
                        "--pattern",
                        &args.pattern,
                        "--seconds",
                        &args.seconds.to_string(),
                        "--out",
                        &args.out.to_string_lossy(),
                    ])),
                    CrtCommand::Restore(_) => {
                        crt_qualification::run(&device_strings(["qualify", "--restore"]))
                    }
                },
                DeviceCommand::Capture { command } => match command {
                    CaptureCommand::Framebuffer(args) => {
                        let mut values = Vec::new();
                        if let Some(output) = &args.output {
                            values
                                .extend(["--output".into(), output.to_string_lossy().into_owned()]);
                        }
                        capture_buffer_at(prepared.config.agent()?, &values)
                    }
                },
                DeviceCommand::Events => agent_cli(&device_strings(["timeline"])),
                DeviceCommand::Fpga { command } => match command {
                    DeviceFpgaCommand::InstallExperimentalAgent(args) => {
                        install_experimental_agent_transaction(
                            &prepared.config,
                            &args.agent,
                            &args.expected_rbf_sha256,
                        )
                    }
                },
            }
        })();
        result.map_err(device_failure)
    }

    pub(crate) fn read_development_manifest(
        &mut self,
    ) -> std::result::Result<String, DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        Ok(remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE).unwrap_or_default())
    }

    pub(crate) fn read_active_runtime(
        &mut self,
    ) -> std::result::Result<ActiveRuntime, DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        Ok(parse_active_runtime_status(
            remote_read(&session, MAIN_STATUS_REMOTE).as_deref(),
        ))
    }

    pub(crate) fn verify_development_platform(&mut self) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        exec_checked(
            &session,
            "development platform verify",
            &installed_platform_verify_command(Layout::Development),
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))
    }

    fn benchmark_profile(
        &mut self,
        operation: impl FnOnce(&NativeDeviceConfig) -> Result<String>,
    ) -> std::result::Result<String, DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        operation(&prepared.config).map_err(device_failure)
    }

    pub(crate) fn verify_input_integrity(
        &mut self,
        output_dir: &Path,
    ) -> std::result::Result<String, DeviceFailure> {
        self.benchmark_profile(|config| verify_installed_input_integrity(config, output_dir))
    }

    pub(crate) fn verify_development_health(&mut self) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        let active =
            parse_active_runtime_status(remote_read(&session, MAIN_STATUS_REMOTE).as_deref());
        if !active.is_development_launcher() {
            return Err(DeviceFailure::Unhealthy(format!(
                "benchmark requires the active development launcher, found {}; run scripts/agent deliver platform",
                active.description()
            )));
        }
        wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45))
            .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))?;
        wait_delivery_health(&session, "dev", Duration::from_secs(10))
            .map_err(|error| DeviceFailure::Unhealthy(error.to_string()))
    }
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
        match agent_request_at(
            config.agent().map_err(device_failure)?,
            "ping",
            json!({}),
            Duration::from_millis(1_500),
        ) {
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

const LOCAL_MAIN_REMOTE: &str = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.main;
const LOCAL_MAIN_MANIFEST_REMOTE: &str =
    mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.manifest;

fn parse_local_main_manifest_text(text: &str) -> Result<BTreeMap<String, String>> {
    platform_manifest_contract::parse(
        text,
        platform_manifest_contract::Layout::Development,
        platform_manifest_contract::ValidationProfile::AgentStrict,
    )
    .map(platform_manifest_contract::ParsedManifest::into_values)
    .map_err(|error| format!("local Main manifest is invalid: {error}").into())
}

fn experimental_fpga_architecture_is_current(diagnostics: &Value) -> bool {
    match diagnostics
        .get("diagnostic_architecture")
        .and_then(Value::as_str)
    {
        Some("scaler-completion-repair-v1") => {
            diagnostics.get("classification").and_then(Value::as_str)
                == Some("repair_transport_ready")
                && diagnostics
                    .pointer("/capabilities/passive_video_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/protocol_version")
                    .and_then(Value::as_u64)
                    == Some(5)
                && diagnostics
                    .pointer("/capabilities/flags")
                    .and_then(Value::as_u64)
                    == Some(0x03ff)
                && diagnostics
                    .pointer("/capabilities/crc")
                    .and_then(Value::as_u64)
                    .is_some()
                && diagnostics
                    .pointer("/presentation_telemetry/magik_ownership")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/presentation_telemetry/lifetime_invariant_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/presentation_telemetry/presented_vblank_count")
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count >= 2)
                && diagnostics
                    .pointer("/presentation_telemetry/active_sequence")
                    .and_then(Value::as_u64)
                    .is_some_and(|sequence| {
                        diagnostics
                            .pointer("/latch_status/active_sequence")
                            .and_then(Value::as_u64)
                            == Some(sequence)
                    })
                && diagnostics
                    .pointer("/presentation_telemetry/crc")
                    .and_then(Value::as_u64)
                    .is_some()
        }
        Some("raw-scaler-boundary-v1") => {
            matches!(
                diagnostics.get("classification").and_then(Value::as_str),
                Some(
                    "raw_scaler_timing_stalled"
                        | "raw_scaler_no_active_video"
                        | "raw_scaler_black"
                        | "raw_scaler_sparse_or_corrupt"
                        | "raw_scaler_active"
                )
            ) && diagnostics
                .pointer("/capabilities/passive_video_observer")
                .and_then(Value::as_bool)
                == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_scheduler_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/raw_scaler_boundary")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/coherence/frame_deltas")
                    .and_then(Value::as_array)
                    .is_some_and(|deltas| deltas.len() == 2)
                && diagnostics
                    .pointer("/raw_scaler_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
        }
        Some("raw-scaler-frame-integrity-v1") => {
            diagnostics.get("classification").and_then(Value::as_str)
                == Some("raw_control_stable_since_baseline")
                && diagnostics
                    .pointer("/capabilities/passive_video_observer")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_scheduler_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/raw_scaler_frame_integrity")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/coherence/records_identical")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/raw_scaler_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
        }
        Some(
            architecture @ ("scaler-fetch-liveness-first-stall-v1"
            | "scaler-fetch-no-request-gates-v1"
            | "scaler-output-scheduler-gates-v1"
            | "scaler-pre-read-scheduler-evidence-v1"
            | "scaler-off-domain-scheduler-snapshot-v1"
            | "scaler-off-domain-scheduler-snapshot-v2"
            | "scaler-off-domain-scheduler-terminal-v3"
            | "scaler-off-domain-scheduler-terminal-v4"
            | "scaler-off-domain-scheduler-terminal-v5"
            | "scaler-off-domain-scheduler-terminal-v6"),
        ) => {
            let scheduler_state = matches!(
                architecture,
                "scaler-fetch-no-request-gates-v1"
                    | "scaler-output-scheduler-gates-v1"
                    | "scaler-pre-read-scheduler-evidence-v1"
                    | "scaler-off-domain-scheduler-snapshot-v1"
                    | "scaler-off-domain-scheduler-snapshot-v2"
                    | "scaler-off-domain-scheduler-terminal-v3"
                    | "scaler-off-domain-scheduler-terminal-v4"
                    | "scaler-off-domain-scheduler-terminal-v5"
                    | "scaler-off-domain-scheduler-terminal-v6"
            );
            matches!(
                diagnostics.get("classification").and_then(Value::as_str),
                Some(
                    "scaler_fetch_normal_liveness"
                        | "scaler_fetch_no_request_seen"
                        | "scaler_fetch_accept_blocked"
                        | "scaler_fetch_first_return_missing"
                        | "scaler_fetch_return_incomplete"
                        | "scaler_fetch_request_cancelled"
                        | "scaler_fetch_reset_stuck"
                        | "scaler_fetch_return_drain_outstanding"
                        | "scaler_fetch_return_drain_release_failed"
                        | "scaler_fetch_return_drain_not_ready"
                        | "scaler_fetch_write_starvation"
                        | "scaler_fetch_read_intent_missing"
                        | "scaler_fetch_acceptance_guard_stuck"
                        | "scaler_fetch_output_request_stopped_after_activity"
                        | "scaler_fetch_output_request_never_started"
                        | "scaler_fetch_scheduler_pending_stuck"
                        | "scaler_output_read_acknowledgement_stuck"
                        | "scaler_output_waitread_state_stuck"
                        | "scaler_output_address_ready_stuck"
                        | "scaler_output_request_toggle_stuck"
                        | "scaler_output_completion_credit_missing"
                        | "scaler_output_copy_start_gate_stuck"
                        | "scaler_output_copy_shift_stuck"
                        | "scaler_output_copy_decrement_stuck"
                        | "scaler_output_copy_terminal_condition_stall"
                        | "scaler_output_read_level_saturated"
                        | "scaler_output_scheduler_state_stuck"
                        | "scaler_pre_read_ack_window_missing"
                        | "scaler_pre_read_output_enable_missing"
                        | "scaler_pre_read_horizontal_sync_edge_missing"
                        | "scaler_pre_read_horizontal_start_missing"
                        | "scaler_pre_read_hsync_state_missing"
                        | "scaler_pre_read_vertical_size_zero"
                        | "scaler_pre_read_vertical_iteration_stuck"
                        | "scaler_pre_read_vertical_decision_missing"
                        | "scaler_pre_read_vertical_pixel_and_carry_gates_closed"
                        | "scaler_pre_read_vertical_pixel_gate_closed"
                        | "scaler_pre_read_vertical_carry_gate_closed"
                        | "scaler_pre_read_address_ready_missing"
                        | "scaler_pre_read_request_issue_missing"
                        | "scaler_pre_read_request_boundary_stuck"
                )
            ) && diagnostics
                .pointer("/capabilities/passive_video_observer")
                .and_then(Value::as_bool)
                == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_scheduler_state")
                    .and_then(Value::as_bool)
                    == Some(scheduler_state)
                && diagnostics
                    .pointer("/capabilities/scaler_fetch_liveness")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_fetch_ordered_signature")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/raw_scaler_ordered_signature")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && (diagnostics
                    .pointer("/coherence/publication_coherent")
                    .and_then(Value::as_bool)
                    == Some(true)
                    || diagnostics
                        .pointer("/coherence/publication_sequence_advancing")
                        .and_then(Value::as_bool)
                        == Some(true))
                && diagnostics
                    .pointer("/coherence/classification_stable")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/scaler_fetch_liveness_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
                && diagnostics
                    .pointer("/scaler_fetch_liveness_state/record_valid")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values.len() == 3
                            && values.iter().all(|value| value.as_bool() == Some(true))
                    })
                && diagnostics
                    .pointer("/scaler_fetch_liveness_state/observer_fault")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values.len() == 3
                            && values.iter().all(|value| value.as_bool() == Some(false))
                    })
        }
        Some("scaler-fetch-ordered-signature-v1") => {
            matches!(
                diagnostics.get("classification").and_then(Value::as_str),
                Some(
                    "scaler_fetch_ordered_stable"
                        | "scaler_fetch_order_changed_requires_static_source_proof"
                )
            ) && diagnostics
                .pointer("/capabilities/passive_video_observer")
                .and_then(Value::as_bool)
                == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_fetch_ordered_signature")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/raw_scaler_ordered_signature")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/coherence/classification_stable")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/scaler_fetch_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
                && diagnostics
                    .pointer("/scaler_fetch_state/fault_flags")
                    .and_then(Value::as_array)
                    .is_some_and(|flags| {
                        flags.len() == 3 && flags.iter().all(|flag| flag.as_u64() == Some(0))
                    })
                && diagnostics
                    .pointer("/scaler_fetch_state/capture_sequence")
                    .and_then(Value::as_array)
                    .is_some_and(|sequences| {
                        sequences.len() == 3
                            && sequences.windows(2).all(|pair| {
                                pair[0].as_u64().zip(pair[1].as_u64()).is_some_and(
                                    |(left, right)| {
                                        let delta = (right as u16).wrapping_sub(left as u16);
                                        delta != 0 && delta <= 0x7fff
                                    },
                                )
                            })
                    })
        }
        Some("raw-scaler-ordered-signature-v3") => {
            matches!(
                diagnostics.get("classification").and_then(Value::as_str),
                Some(
                    "raw_scaler_ordered_stable"
                        | "raw_scaler_order_changed_requires_static_source_proof"
                )
            ) && diagnostics
                .pointer("/capabilities/passive_video_observer")
                .and_then(Value::as_bool)
                == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_scheduler_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/scaler_pipeline_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/scaler_copy_retirement")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/raw_scaler_ordered_signature")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/coherence/classification_stable")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/raw_scaler_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
                && diagnostics
                    .pointer("/raw_scaler_state/frame_sequence")
                    .and_then(Value::as_array)
                    .is_some_and(|sequences| {
                        sequences.len() == 3
                            && sequences.windows(2).all(|pair| {
                                pair[0].as_u64().zip(pair[1].as_u64()).is_some_and(
                                    |(left, right)| {
                                        let delta = (right as u16).wrapping_sub(left as u16);
                                        delta != 0 && delta <= 0x7fff
                                    },
                                )
                            })
                    })
        }
        Some("scaler-copy-retirement-v1") => {
            diagnostics.get("classification").and_then(Value::as_str)
                == Some("scaler_copy_retirement_active")
                && diagnostics
                    .pointer("/capabilities/passive_video_observer")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/scaler_scheduler_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/scaler_pipeline_state")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/capabilities/scaler_copy_retirement")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pixel_observer")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/capabilities/pll_observer")
                    .and_then(Value::as_bool)
                    == Some(false)
                && diagnostics
                    .pointer("/coherence/three_samples_valid")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/coherence/classification_stable")
                    .and_then(Value::as_bool)
                    == Some(true)
                && diagnostics
                    .pointer("/raw_scaler_state/raw_samples")
                    .and_then(Value::as_array)
                    .is_some_and(|samples| samples.len() == 3)
        }
        _ => false,
    }
}

fn experimental_fpga_evidence_is_current(diagnostics: &Value) -> bool {
    experimental_fpga_transport_is_operational(diagnostics)
        && experimental_fpga_architecture_is_current(diagnostics)
        && diagnostics.get("coherent").and_then(Value::as_bool) == Some(true)
}

fn experimental_fpga_transport_is_operational(diagnostics: &Value) -> bool {
    diagnostics.get("schema").and_then(Value::as_str)
        == Some("mister-magik-fpga-video-diagnostics-v2")
        && diagnostics.get("available").and_then(Value::as_bool) == Some(true)
        && diagnostics.get("sink_visibility").and_then(Value::as_str) == Some("unobserved")
        && diagnostics
            .pointer("/coherence/latch_ownership_stable")
            .and_then(Value::as_bool)
            == Some(true)
        && diagnostics
            .pointer("/coherence/launcher_state_stable")
            .and_then(Value::as_bool)
            == Some(true)
        && diagnostics
            .pointer("/coherence/ownership_check_error")
            .is_some_and(Value::is_null)
        && diagnostics
            .get("owner_epoch_before")
            .and_then(Value::as_u64)
            .is_some_and(|before| {
                before > 0
                    && diagnostics.get("owner_epoch_after").and_then(Value::as_u64) == Some(before)
            })
        && diagnostics
            .pointer("/latch_status/flags")
            .and_then(Value::as_u64)
            .is_some_and(|flags| {
                flags & (1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP) != 0
            })
        && diagnostics
            .pointer("/latch_status/active_width")
            .and_then(Value::as_u64)
            .is_some_and(|width| width > 0)
        && diagnostics
            .pointer("/latch_status/active_height")
            .and_then(Value::as_u64)
            .is_some_and(|height| height > 0)
        && diagnostics
            .pointer("/latch_status/active_stride")
            .and_then(Value::as_u64)
            .is_some_and(|stride| {
                diagnostics
                    .pointer("/latch_status/active_width")
                    .and_then(Value::as_u64)
                    .is_some_and(|width| stride >= width.saturating_mul(2))
            })
        && diagnostics
            .pointer("/latch_status/crc")
            .and_then(Value::as_u64)
            .is_some()
}

fn experimental_fpga_activation_status(session: &Session) -> Result<(u64, u64, i64, u64)> {
    let main_status = remote_read(session, MAIN_STATUS_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .ok_or("experimental FPGA activation has no Main status")?;
    if main_status.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive")
        || main_status.get("executable_path").and_then(Value::as_str) != Some(LOCAL_MAIN_REMOTE)
        || main_status.get("fpga_owner").and_then(Value::as_str) != Some("magik")
        || main_status
            .get("launcher_ready_phase")
            .and_then(Value::as_str)
            != Some("ready")
    {
        return Err(
            "experimental FPGA activation requires stable Dev LauncherActive ownership".into(),
        );
    }
    let generation = main_status
        .get("main_generation")
        .and_then(Value::as_u64)
        .ok_or("experimental FPGA activation has no Main generation")?;
    let main_pid = main_status
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0)
        .ok_or("experimental FPGA activation has no Main pid")?;
    let launcher_pid = main_status
        .get("launcher_pid")
        .and_then(Value::as_i64)
        .filter(|pid| *pid > 0)
        .ok_or("experimental FPGA activation has no active launcher")?;
    let owner_epoch = main_status
        .get("fpga_owner_epoch")
        .and_then(Value::as_u64)
        .filter(|epoch| *epoch > 0)
        .ok_or("experimental FPGA activation has no FPGA owner epoch")?;
    Ok((generation, main_pid, launcher_pid, owner_epoch))
}

fn experimental_agent_transaction_remote() -> String {
    installed_layout::app_path(Layout::Development, "experimental-agent.delivery-state")
        .expect("static installed path")
}

fn validate_experimental_agent(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "experimental device-agent artifact is missing {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.len() < 20 || metadata.len() > 32 * 1024 * 1024 {
        return Err(format!(
            "experimental device-agent artifact has an invalid size: {}",
            path.display()
        )
        .into());
    }
    let mut header = [0_u8; 20];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("cannot read experimental device-agent ELF header: {error}"))?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    if &header[..4] != b"\x7fELF" || header[4] != 1 || header[5] != 1 || machine != 40 {
        return Err(format!(
            "experimental device-agent is not a 32-bit little-endian ARM ELF: {}",
            path.display()
        )
        .into());
    }
    file_sha256(path.to_path_buf())
}

fn install_experimental_agent_transaction(
    config: &NativeDeviceConfig,
    agent: &Path,
    expected_rbf_sha256: &str,
) -> Result<()> {
    if expected_rbf_sha256.len() != 64
        || !expected_rbf_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("expected experimental RBF SHA-256 is not canonical lowercase hex".into());
    }
    let agent_sha256 = validate_experimental_agent(agent)?;
    let transaction = experimental_agent_transaction_remote();
    let remote = DEVELOPMENT_AGENT_REMOTE.as_str();
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "installed Dev platform verification before experimental agent install",
        &installed_platform_verify_command(Layout::Development),
    )?;
    let manifest_text = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("installed Dev manifest is missing")?;
    let manifest = parse_local_main_manifest_text(&manifest_text)?;
    if manifest["latch_rbf_sha256"] != expected_rbf_sha256 {
        return Err("installed Dev RBF does not match the experimental agent transaction".into());
    }
    experimental_fpga_activation_status(&session)?;
    if let Some(state) = remote_read(&session, &transaction) {
        if state.trim() != "activating" {
            return Err(format!(
                "experimental device-agent transaction requires reconciliation: {}",
                state.trim()
            )
            .into());
        }
        exec_checked(
            &session,
            "experimental device-agent reconciled hash",
            &format!(
                "test \"$(sha256sum {remote} | awk '{{print $1}}')\" = {agent_hash}",
                remote = sh(remote),
                agent_hash = sh(&agent_sha256),
            ),
        )?;
        let diagnostics = agent_request_at(
            config.agent()?,
            "diagnostics",
            json!({}),
            Duration::from_secs(5),
        )?;
        let evidence = diagnostics
            .response
            .pointer("/result/fpga_video_diagnostics")
            .ok_or("reconciled experimental device-agent returned no FPGA evidence")?;
        if !experimental_agent_preload_evidence_accepted(evidence) {
            return Err(format!(
                "reconciled experimental device-agent is not compatible with the installed diagnostic RBF: {evidence}"
            )
            .into());
        }
        exec_checked(
            &session,
            "experimental device-agent reconciled commit",
            &format!(
                "rm -f {remote}.delivery-rollback {remote}.upload {transaction}; sync",
                remote = sh(remote),
                transaction = sh(&transaction),
            ),
        )?;
        return Ok(());
    }
    exec_checked(
        &session,
        "experimental device-agent snapshot",
        &format!(
            "set -eu; test ! -e {transaction}; test ! -e {remote}.delivery-rollback; rm -f {remote}.upload; cp -p {remote} {remote}.delivery-rollback; printf 'snapshot\\n' > {transaction}; sync",
            transaction = sh(&transaction),
            remote = sh(remote),
        ),
    )?;
    let install = (|| -> Result<()> {
        put(&session, agent, &format!("{remote}.upload"))?;
        exec_checked(
            &session,
            "experimental device-agent activation",
            &format!(
                "set -eu; test \"$(sha256sum {remote}.upload | awk '{{print $1}}')\" = {agent_hash}; chmod 755 {remote}.upload; printf 'activating\\n' > {transaction}; mv -f {remote}.upload {remote}; sync",
                remote = sh(remote),
                agent_hash = sh(&agent_sha256),
                transaction = sh(&transaction),
            ),
        )?;
        drop(session);
        one_shot_recovery_reboot_wait(config).map_err(|error| format!("{error:?}"))?;
        let verify = connect_with(&config.connection, 10)?;
        exec_checked(
            &verify,
            "experimental device-agent installed hash",
            &format!(
                "test \"$(sha256sum {remote} | awk '{{print $1}}')\" = {agent_hash}",
                remote = sh(remote),
                agent_hash = sh(&agent_sha256),
            ),
        )?;
        let installed_manifest = remote_read(&verify, LOCAL_MAIN_MANIFEST_REMOTE)
            .ok_or("installed Dev manifest is missing after experimental agent reboot")?;
        if parse_local_main_manifest_text(&installed_manifest)?["latch_rbf_sha256"]
            != expected_rbf_sha256
        {
            return Err("experimental RBF identity changed during device-agent reboot".into());
        }
        verify_delivery_health(config).map_err(|error| format!("{error:?}"))?;
        let diagnostics = agent_request_at(
            config.agent()?,
            "diagnostics",
            json!({}),
            Duration::from_secs(5),
        )?;
        let evidence = diagnostics
            .response
            .pointer("/result/fpga_video_diagnostics")
            .ok_or("experimental device-agent returned no FPGA evidence")?;
        if !experimental_agent_preload_evidence_accepted(evidence) {
            return Err(format!(
                "experimental device-agent is not compatible with the installed diagnostic RBF: {evidence}"
            )
            .into());
        }
        Ok(())
    })();
    if let Err(error) = install {
        let rollback = (|| -> Result<()> {
            let rollback = connect_with(&config.connection, 10)?;
            exec_checked(
                &rollback,
                "experimental device-agent rollback",
                &format!(
                    "set -eu; test -f {remote}.delivery-rollback; cp -p {remote}.delivery-rollback {remote}.upload; chmod 755 {remote}.upload; mv -f {remote}.upload {remote}; printf 'rolled-back\\n' > {transaction}; sync",
                    remote = sh(remote),
                    transaction = sh(&transaction),
                ),
            )?;
            drop(rollback);
            one_shot_recovery_reboot_wait(config).map_err(|error| format!("{error:?}"))?;
            verify_delivery_health(config).map_err(|failure| format!("{failure:?}"))?;
            let cleanup = connect_with(&config.connection, 10)?;
            exec_checked(
                &cleanup,
                "experimental device-agent rollback cleanup",
                &format!(
                    "rm -f {remote}.delivery-rollback {remote}.upload {transaction}; sync",
                    remote = sh(remote),
                    transaction = sh(&transaction),
                ),
            )?;
            Ok(())
        })();
        return match rollback {
            Ok(()) => Err(format!(
                "experimental device-agent install failed ({error}); rollback=complete"
            )
            .into()),
            Err(rollback) => Err(format!(
                "experimental device-agent install failed ({error}); rollback failed ({rollback})"
            )
            .into()),
        };
    }
    let commit = connect_with(&config.connection, 10)?;
    exec_checked(
        &commit,
        "experimental device-agent commit",
        &format!(
            "rm -f {remote}.delivery-rollback {remote}.upload {transaction}; sync",
            remote = sh(remote),
            transaction = sh(&transaction),
        ),
    )?;
    Ok(())
}

fn experimental_raw_scaler_evidence_available(evidence: &Value) -> bool {
    experimental_fpga_architecture_is_current(evidence)
        && evidence.get("available").and_then(Value::as_bool) == Some(true)
        && evidence.get("sink_visibility").and_then(Value::as_str) == Some("unobserved")
        && evidence
            .pointer("/capabilities/passive_video_observer")
            .and_then(Value::as_bool)
            == Some(true)
        && [
            "/raw_scaler_state/raw_samples",
            "/scaler_fetch_state/raw_samples",
            "/scaler_fetch_liveness_state/raw_samples",
        ]
        .iter()
        .any(|path| {
            evidence
                .pointer(path)
                .and_then(Value::as_array)
                .is_some_and(|samples| samples.len() == 3)
        })
}

fn scaler_fetch_liveness_preload_evidence_available(evidence: &Value) -> bool {
    evidence.get("schema").and_then(Value::as_str) == Some("mister-magik-fpga-video-diagnostics-v2")
        && evidence
            .get("diagnostic_architecture")
            .and_then(Value::as_str)
            .is_some_and(|architecture| {
                matches!(
                    architecture,
                    "scaler-fetch-liveness-first-stall-v1"
                        | "scaler-fetch-no-request-gates-v1"
                        | "scaler-output-scheduler-gates-v1"
                        | "scaler-pre-read-scheduler-evidence-v1"
                        | "scaler-off-domain-scheduler-snapshot-v1"
                        | "scaler-off-domain-scheduler-snapshot-v2"
                        | "scaler-off-domain-scheduler-terminal-v3"
                        | "scaler-off-domain-scheduler-terminal-v4"
                        | "scaler-off-domain-scheduler-terminal-v5"
                        | "scaler-off-domain-scheduler-terminal-v6"
                )
            })
        && evidence.get("available").and_then(Value::as_bool) == Some(true)
        && evidence.get("sink_visibility").and_then(Value::as_str) == Some("unobserved")
        && evidence
            .pointer("/capabilities/passive_video_observer")
            .and_then(Value::as_bool)
            == Some(true)
        && evidence
            .pointer("/capabilities/scaler_fetch_liveness")
            .and_then(Value::as_bool)
            == Some(true)
        && (evidence
            .pointer("/coherence/publication_coherent")
            .and_then(Value::as_bool)
            == Some(true)
            || evidence
                .pointer("/coherence/publication_sequence_advancing")
                .and_then(Value::as_bool)
                == Some(true))
        && evidence
            .pointer("/coherence/latch_ownership_stable")
            .and_then(Value::as_bool)
            == Some(true)
        && evidence
            .pointer("/coherence/launcher_state_stable")
            .and_then(Value::as_bool)
            == Some(true)
        && evidence
            .pointer("/coherence/ownership_check_error")
            .is_some_and(Value::is_null)
        && evidence
            .pointer("/scaler_fetch_liveness_state/raw_samples")
            .and_then(Value::as_array)
            .is_some_and(|samples| samples.len() == 3)
        && evidence
            .pointer("/scaler_fetch_liveness_state/record_valid")
            .and_then(Value::as_array)
            .is_some_and(|valid| {
                valid.len() == 3 && valid.iter().any(|value| value.as_bool() == Some(true))
            })
        && evidence
            .pointer("/scaler_fetch_liveness_state/observer_fault")
            .and_then(Value::as_array)
            .is_some_and(|faults| {
                faults.len() == 3 && faults.iter().all(|value| value.as_bool() == Some(false))
            })
}

fn experimental_agent_preload_evidence_accepted(evidence: &Value) -> bool {
    experimental_fpga_evidence_is_current(evidence)
        || experimental_raw_scaler_evidence_available(evidence)
        || scaler_fetch_liveness_preload_evidence_available(evidence)
        || (evidence.get("available").and_then(Value::as_bool) == Some(false)
            && evidence.get("coherent").and_then(Value::as_bool) == Some(false)
            && evidence.get("schema").and_then(Value::as_str)
                == Some("mister-magik-fpga-video-diagnostics-v1")
            && evidence.get("classification").and_then(Value::as_str) == Some("unclassified")
            && matches!(
                evidence.get("reason").and_then(Value::as_str),
                Some(
                    "read passive FPGA video diagnostics: unsupported raw scaler state schema 1"
                        | "read passive FPGA video diagnostics: unsupported raw scaler state schema 2"
                        | "read passive FPGA video diagnostics: unsupported raw scaler state schema 3"
                        | "read passive FPGA video diagnostics: unsupported raw scaler state schema 4"
                        | "read passive FPGA video diagnostics: unsupported raw scaler state \
                           schema 5"
                )
            ))
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

fn install_prepared_device_environment(config: &NativeDeviceConfig) {
    // Rust 2024 marks process-environment mutation unsafe because concurrent
    // readers in foreign code may race it. Device resolution runs once, before
    // SSH/libssh2 or any worker thread is started by an operator command.
    unsafe {
        env::set_var("MISTER_IP", config.connection.host());
        env::set_var("MISTER_DEVICE_ID", &config.device_id);
    }
}

fn device_strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}

fn main_process_name(layout: Layout) -> &'static str {
    Path::new(installed_layout::paths(layout).main)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("schema-owned Main path has a file name")
}

fn named_installed_layout(layout: &str) -> Result<(Layout, &'static str)> {
    let layout = match layout {
        "dev" => Layout::Development,
        "public" => Layout::Public,
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok((layout, main_process_name(layout)))
}

fn delivery_health_command(layout: &str) -> Result<String> {
    let (layout, main) = named_installed_layout(layout)?;
    let directory = installed_layout::paths(layout).root;
    Ok(format!(
        "set -eu; health_check=initializing; trap 'rc=$?; if test \"$rc\" -ne 0; then printf \"delivery_health_failure_tsv\\tcheck=%s\\trc=%s\\n\" \"$health_check\" \"$rc\" >&2; fi' EXIT; health_check=main-process; pidof {main} >/dev/null; health_check=launcher-process; pidof mister-magik-fb >/dev/null; health_check=scanout-module; grep -q '^mister_magik_scanout_slots ' /proc/modules; health_check=scanout-device; test -c /dev/mister-magik-scanout-slots; health_check=latch-readiness; report=$({directory}/mister-magik-fb latch-readiness-report); printf '%s\\n' \"$report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready'; health_check=launcher-env-clear; test ! -e {directory}/launcher.env; health_check=rebuild-clear; test ! -e {directory}/rebuild-on-next-boot; health_check=fault-launcher-env-clear; test ! -e /tmp/mister-magik/fs-fault-launcher.env; health_check=fault-session-clear; test ! -e /tmp/mister-magik/fs-fault-session; health_check=fault-json-clear; test ! -e /tmp/mister-magik/fs-fault.json; health_check=complete; trap - EXIT; printf 'delivery_health_tsv\\tvalid=1\\n'"
    ))
}

fn parse_active_runtime_status(status: Option<&str>) -> ActiveRuntime {
    let status = status.and_then(|status| serde_json::from_str::<Value>(status).ok());
    ActiveRuntime::new(
        status
            .as_ref()
            .and_then(|status| status.get("executable_path"))
            .and_then(Value::as_str),
        status
            .as_ref()
            .and_then(|status| status.get("launcher_state"))
            .and_then(Value::as_str),
    )
}

fn wait_delivery_health(session: &Session, layout: &str, timeout: Duration) -> Result<()> {
    let command = delivery_health_command(layout)?;
    let started = Instant::now();
    let mut attempts = 0_u32;
    loop {
        attempts = attempts.saturating_add(1);
        let output = match exec(session, &command, true) {
            Ok(output) => output,
            Err(_) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "delivery health transport failed after {attempts} attempts and {}ms: {error}",
                    started.elapsed().as_millis()
                )
                .into());
            }
        };
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

fn release_arming_cleanup_command() -> &'static str {
    static COMMAND: LazyLock<String> = LazyLock::new(|| {
        let paths = installed_layout::arming_paths();
        format!(
            "rm -f {} {} {} {} {} /tmp/mister-magik/latch-v5-qualification-control.tsv /tmp/mister-magik/latch-v5-qualification-control.tsv.tmp /tmp/mister-magik/latch-v5-qualification-state.json {} {}; rm -rf /tmp/mister-magik/latch-v5-catalog",
            sh(&paths[0]),
            sh(&paths[1]),
            sh(&paths[2]),
            sh(&paths[3]),
            sh(&paths[4]),
            sh(&paths[5]),
            sh(&paths[6]),
        )
    });
    COMMAND.as_str()
}

fn arming_status() -> Result<()> {
    let session = connect(10)?;
    let paths = installed_layout::arming_paths()
        .iter()
        .map(|path| sh(path))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!(
        "set -eu; found=0; for path in {paths}; do if test -e \"$path\"; then printf 'armed=%s\\n' \"$path\"; found=1; fi; done; test \"$found\" = 1 || echo arming=clear"
    );
    let output = exec(&session, &command, false)?;
    if let Some(message) = exec_failure_message("arming status", &output) {
        return Err(message.into());
    }
    print!("{}", output.stdout);
    Ok(())
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
        "cleanup() {{ trap - EXIT HUP INT TERM; {resume}; }}; trap cleanup EXIT HUP INT TERM; set -eu; test -x {gui}; {diagnostic}MISTER_MAGIK_RUNTIME_SETTINGS_V1={} {gui} ui crt_trial 30 >/tmp/mister-magik-crt_trial.log 2>&1",
        sh(runtime_settings),
        gui = sh(DEVELOPMENT_GUI_REMOTE),
    )
}

fn parse_crt_trial_status(output: &str) -> Result<&str> {
    const MARKERS: [&str; 3] = [
        "crt_trial_status_v2 schema=2 ",
        "crt_trial_status_v3 schema=3 ",
        "crt_trial_status_v5 schema=5 ",
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

const INPUT_INTEGRITY_TRACE_REMOTE: &str = "/tmp/mister-magik/input-integrity-trace.json";
const INPUT_INTEGRITY_EXPECTED_PRESSES: u64 = 109;
const STEADY_STATE_CATALOG_REFRESH_POLICY: &str = "default";

fn verify_installed_input_integrity(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    fs::create_dir_all(output_dir)?;
    let session = connect_with(&config.connection, 10)?;
    let main_before: Value = serde_json::from_str(
        &remote_read(&session, MAIN_STATUS_REMOTE).ok_or("Main status is missing")?,
    )?;
    let input_proxy_protocol = main_before
        .get("input_proxy_protocol")
        .and_then(Value::as_u64)
        .ok_or("input integrity Main status omitted the input proxy protocol")?;
    if !matches!(input_proxy_protocol, 2 | 3) {
        return Err(format!(
            "input integrity requires Main proxy protocol v2 or v3, got {input_proxy_protocol}"
        )
        .into());
    }
    let idle = run_input_integrity_scenario(
        &session,
        "idle",
        STEADY_STATE_CATALOG_REFRESH_POLICY,
        None,
        false,
        "down",
    )?;
    let stress = run_input_integrity_scenario(
        &session,
        "cpu-stall",
        STEADY_STATE_CATALOG_REFRESH_POLICY,
        Some(500),
        true,
        "down",
    )?;
    let horizontal = run_input_integrity_scenario(
        &session,
        "horizontal-idle",
        STEADY_STATE_CATALOG_REFRESH_POLICY,
        None,
        false,
        "right",
    )?;
    let launcher = read_launcher_status(&session)?;
    let main_after: Value = serde_json::from_str(
        &remote_read(&session, MAIN_STATUS_REMOTE).ok_or("Main status is missing after run")?,
    )?;
    let counter_delta = |field: &str| {
        main_after[field]
            .as_u64()
            .unwrap_or(u64::MAX)
            .saturating_sub(main_before[field].as_u64().unwrap_or(0))
    };
    let proxy_write_failures = counter_delta("input_proxy_write_failures");
    let journal_overflows = counter_delta("input_proxy_journal_overflows");
    let sequence_gaps = counter_delta("input_proxy_desyncs");
    let observed_latch_drops = launcher["latch_drop_count"].as_u64().unwrap_or(u64::MAX);
    let observed_dropped_frames = launcher
        .pointer("/frame_budget/physical_refresh/dropped_frames")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let status = if proxy_write_failures == 0 && journal_overflows == 0 && sequence_gaps == 0 {
        "passed"
    } else {
        "failed"
    };
    let summary = json!({
        "schema": "mister-magik-input-integrity-v2",
        "status": status,
        "protocol": input_proxy_protocol,
        "path": format!("uinput -> Main mapping -> Main proxy v{input_proxy_protocol} -> kernel evdev -> InputCapture -> InputRouter"),
        "scenarios": [idle, stress, horizontal],
        "expected_initial_presses_per_scenario": INPUT_INTEGRITY_EXPECTED_PRESSES,
        "lost_actions": 0,
        "duplicated_actions": 0,
        "proxy_write_failures": proxy_write_failures,
        "journal_overflows": journal_overflows,
        "sequence_gaps": sequence_gaps,
        "observed_dropped_frames": observed_dropped_frames,
        "observed_latch_drops": observed_latch_drops,
        "cadence_is_not_an_input_integrity_gate": true,
        "attended_checks_required": true,
    });
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    launcher_restart(
        &session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            timeout_secs: 45,
            ..LauncherRestartOptions::default()
        },
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
}

fn run_input_integrity_driver(session: &Session, load: bool, action: &str) -> Result<()> {
    let mode = if action == "right" {
        "qualification-right"
    } else if load {
        "qualification-load"
    } else {
        "qualification"
    };
    exec_checked(
        session,
        "input integrity sequence",
        &format!(
            "{} {mode}",
            development_gui_command("input-integrity-driver")
        ),
    )
}

fn run_input_integrity_scenario(
    session: &Session,
    label: &str,
    catalog_refresh: &str,
    stall_ms: Option<u64>,
    cpu_load: bool,
    action: &str,
) -> Result<Value> {
    let mut env_vars = vec![
        ("MISTER_CATALOG_REFRESH".into(), catalog_refresh.into()),
        ("MISTER_LAUNCHER_START_SCREEN".into(), "settings".into()),
        ("MISTER_INPUT_INTEGRITY_TRACE".into(), "1".into()),
    ];
    if let Some(stall_ms) = stall_ms {
        env_vars.push((
            "MISTER_INPUT_INTEGRITY_STALL_MS".into(),
            stall_ms.to_string(),
        ));
    }
    restart_launcher_with_one_shot_env(
        session,
        LauncherRestartOptions {
            env_vars,
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    let ready = wait_input_integrity_launcher(session, Duration::from_secs(45))?;
    if catalog_refresh == "force"
        && ready.get("catalog_refresh_done").and_then(Value::as_bool) != Some(false)
    {
        return Err("input integrity stress scenario missed the active catalog refresh".into());
    }
    run_input_integrity_driver(session, cpu_load, action)?;
    let trace = wait_input_integrity_trace(session, Duration::from_secs(5))?;
    validate_input_integrity_trace(&trace, stall_ms.is_none(), action)?;
    Ok(json!({
        "label": label,
        "catalog_refresh": catalog_refresh,
        "cpu_load": cpu_load,
        "action": action,
        "ui_stall_ms": stall_ms.unwrap_or(0),
        "trace": trace,
    }))
}

fn wait_input_integrity_trace(session: &Session, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    let mut last_trace = None;
    loop {
        if let Some(raw) = remote_read(session, INPUT_INTEGRITY_TRACE_REMOTE)
            && let Ok(trace) = serde_json::from_str::<Value>(&raw)
        {
            if trace.get("initial_presses").and_then(Value::as_u64)
                >= Some(INPUT_INTEGRITY_EXPECTED_PRESSES)
                && trace.get("releases").and_then(Value::as_u64)
                    >= Some(INPUT_INTEGRITY_EXPECTED_PRESSES)
            {
                return Ok(trace);
            }
            last_trace = Some(trace);
        }
        if started.elapsed() >= timeout {
            let detail = last_trace.as_ref().map_or_else(
                || "trace file was absent or invalid".to_string(),
                |trace| {
                    format!(
                        "presses={} releases={} repeats={} final_down_held={} final_right_held={}",
                        trace["initial_presses"].as_u64().unwrap_or(0),
                        trace["releases"].as_u64().unwrap_or(0),
                        trace["repeats"].as_u64().unwrap_or(0),
                        trace["final_down_held"].as_bool().unwrap_or(false),
                        trace["final_right_held"].as_bool().unwrap_or(false),
                    )
                },
            );
            return Err(
                format!("timed out waiting for the input integrity trace: {detail}").into(),
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn validate_input_integrity_trace(
    trace: &Value,
    enforce_latency: bool,
    expected_action: &str,
) -> Result<()> {
    let final_held_field = if expected_action == "right" {
        "final_right_held"
    } else {
        "final_down_held"
    };
    if trace.get("schema").and_then(Value::as_str) != Some("mister-magik-input-integrity-trace-v1")
        || trace.get("initial_presses").and_then(Value::as_u64)
            != Some(INPUT_INTEGRITY_EXPECTED_PRESSES)
        || trace.get("releases").and_then(Value::as_u64) != Some(INPUT_INTEGRITY_EXPECTED_PRESSES)
        || trace.get(final_held_field).and_then(Value::as_bool) != Some(false)
        || trace.get("repeats").and_then(Value::as_u64).unwrap_or(0) == 0
        || trace
            .get("queue_high_water")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            >= 1_024
        || (enforce_latency
            && trace
                .get("dispatch_p99_us")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
                > 16_667)
    {
        return Err(format!(
            "input integrity trace summary failed: schema={} presses={} releases={} repeats={} final_held_field={final_held_field} final_held={} queue_high_water={} dispatch_p99_us={} latency_gate={enforce_latency}",
            trace["schema"].as_str().unwrap_or("missing"),
            trace["initial_presses"].as_u64().unwrap_or(0),
            trace["releases"].as_u64().unwrap_or(0),
            trace["repeats"].as_u64().unwrap_or(0),
            trace[final_held_field].as_bool().unwrap_or(false),
            trace["queue_high_water"].as_u64().unwrap_or(0),
            trace["dispatch_p99_us"].as_u64().unwrap_or(0),
        )
        .into());
    }
    let physical: Vec<&Value> = trace["records"]
        .as_array()
        .ok_or("input integrity trace has no records")?
        .iter()
        .filter(|record| matches!(record["kind"].as_str(), Some("initial" | "release")))
        .collect();
    if physical.len() != (INPUT_INTEGRITY_EXPECTED_PRESSES * 2) as usize {
        return Err("input integrity trace event count is wrong".into());
    }
    for (pair_index, pair) in physical.as_chunks::<2>().0.iter().enumerate() {
        let press = pair[0];
        let release = pair[1];
        if press["kind"] != "initial"
            || press["phase"] != "pressed"
            || release["kind"] != "release"
            || release["phase"] != "released"
            || press["action"] != expected_action
            || release["action"] != expected_action
            || press["press_id"] != release["press_id"]
            || release["sequence"].as_u64()
                != press["sequence"].as_u64().map(|sequence| sequence + 1)
            || (pair_index > 0
                && press["sequence"].as_u64()
                    != physical[pair_index * 2 - 1]["sequence"]
                        .as_u64()
                        .map(|sequence| sequence + 1))
        {
            return Err(format!("input integrity trace is invalid at pair {pair_index}").into());
        }
    }
    Ok(())
}

fn wait_input_integrity_launcher(session: &Session, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        let status = read_launcher_status(session)?;
        if status.get("catalog_ready").and_then(Value::as_bool) == Some(true)
            && status.get("return_screen").and_then(Value::as_str) == Some("settings")
        {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err("input integrity launcher did not become ready on Settings".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn restart_launcher_with_one_shot_env(
    session: &Session,
    options: LauncherRestartOptions,
) -> Result<()> {
    let previous = wait_launcher_ready(session, Instant::now(), Duration::from_secs(5))?;
    stage_one_shot_launcher_env(session, &options)?;
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
    let clear_result = clear_one_shot_launcher_env(session, &options.remote_env);
    match (restart_result, clear_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => {
            Err(format!("one-shot launcher env cleanup failed: {error}").into())
        }
        (Err(restart_error), Err(clear_error)) => Err(format!(
            "{restart_error}; one-shot launcher env cleanup failed: {clear_error}"
        )
        .into()),
    }
}

fn stage_one_shot_launcher_env(session: &Session, options: &LauncherRestartOptions) -> Result<()> {
    if options.clear_env || options.env_vars.is_empty() {
        return Err("one-shot launcher restart requires environment variables".into());
    }
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
    Ok(())
}

fn clear_one_shot_launcher_env(session: &Session, remote_env: &str) -> Result<()> {
    prepare_launcher_env(
        session,
        &LauncherRestartOptions {
            clear_env: true,
            remote_env: remote_env.to_string(),
            ..LauncherRestartOptions::default()
        },
    )
    .map(|_| ())
}

fn one_shot_launcher_env_text(vars: &[(String, String)], remote_env: &str) -> String {
    let mut text = launcher_env_text(vars);
    text.push_str("rm -f ");
    text.push_str(&shell_export_quote(remote_env));
    text.push('\n');
    text
}

fn read_launcher_status(session: &Session) -> Result<Value> {
    let text = remote_read(session, SLINT_STATUS_REMOTE).ok_or("launcher status is missing")?;
    serde_json::from_str(&text).map_err(Into::into)
}

fn reboot_remote_command(mode: RebootMode) -> String {
    match mode {
        RebootMode::Supervised => acknowledged_main_command("mister_magik_reboot"),
        RebootMode::Raw => RAW_REBOOT_REMOTE_CMD.to_string(),
    }
}

fn issue_reboot(sess: &Session, mode: RebootMode) -> Result<String> {
    let command = reboot_remote_command(mode);
    let out = exec(sess, &command, true)?;
    if let Some(message) = exec_failure_message("reboot request", &out) {
        return Err(message.into());
    }
    let mode = out.stdout.trim();
    if mode.is_empty() {
        Ok("unknown".to_string())
    } else {
        Ok(mode.to_string())
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
                let tag = e.name().as_ref().to_owned();
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
                let tag = e.name().as_ref().to_owned();
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
                    let text = e.xml10_content().into_owned();
                    match field.as_str() {
                        "description" => machine.title = text,
                        "year" => machine.year = Some(text),
                        "manufacturer" => machine.manufacturer = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = e.name().as_ref().to_owned();
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

#[cfg(test)]
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
                let tag = e.name().as_ref().to_owned();
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
                let tag = e.name().as_ref().to_owned();
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
                    let text = e.xml10_content().into_owned();
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
                let tag = e.name().as_ref().to_owned();
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
fn apply_mame_display(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.display_type = attr_value(e, b"type");
    machine.rotate = attr_value(e, b"rotate").and_then(|value| value.parse().ok());
    machine.display_width = attr_value(e, b"width").and_then(|value| value.parse().ok());
    machine.display_height = attr_value(e, b"height").and_then(|value| value.parse().ok());
    machine.refresh_hz = attr_value(e, b"refresh").and_then(|value| value.parse().ok());
}

#[cfg(test)]
fn apply_mame_input(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.players = attr_value(e, b"players").and_then(|value| value.parse().ok());
    machine.coins = attr_value(e, b"coins").and_then(|value| value.parse().ok());
}

#[cfg(test)]
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

#[cfg(test)]
fn apply_mame_driver(machine: &mut MameMachine, e: &BytesStart<'_>) {
    machine.driver_status = attr_value(e, b"status");
    machine.emulation_status = attr_value(e, b"emulation");
    machine.savestate = attr_value(e, b"savestate");
}

#[cfg(test)]
fn attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref().as_bytes() == key)
        .map(|attr| attr.value.as_ref().to_owned())
}

#[cfg(test)]
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

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn agent_cli(args: &[String]) -> Result<()> {
    let subcommand = args.first().map(String::as_str).unwrap_or("status");
    match subcommand {
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
        "diagnostics" => {
            agent_diagnostics(&args[1..])?;
        }
        "magik" => {
            agent_magik(&args[1..])?;
        }
        "reboot-wait" => {
            agent_reboot_wait(&args[1..])?;
        }
        other => return Err(format!("unknown agent subcommand: {other}").into()),
    }
    Ok(())
}

struct PngCapture {
    result: Value,
    png: Vec<u8>,
}

struct PendingCaptureArtifact {
    label: &'static str,
    path: PathBuf,
    png: Vec<u8>,
}

struct CaptureArtifactLink {
    label: &'static str,
    path: PathBuf,
}

fn capture_buffer_at(agent: &AgentEndpoint, args: &[String]) -> Result<()> {
    validate_capture_buffer_args(args)?;
    let output = option_value(args, "--output");
    let artifacts = capture_buffer_bundle_at(agent, output.as_deref())?;
    print_capture_artifacts(&artifacts);
    Ok(())
}

fn capture_buffer_bundle_at(
    agent: &AgentEndpoint,
    requested_stem: Option<&str>,
) -> Result<Vec<CaptureArtifactLink>> {
    let capture = request_framebuffer_png_at(agent)?;
    let artifacts = write_capture_bundle(&capture, requested_stem)?;
    eprintln!(
        "framebuffer capture source={}",
        capture_source_label(&capture.result)?
    );
    Ok(artifacts)
}

fn print_capture_artifacts(artifacts: &[CaptureArtifactLink]) {
    for artifact in artifacts {
        if io::stdout().is_terminal() {
            println!("{}: {}", artifact.label, artifact.path.display());
        } else {
            println!("[{}](<{}>)", artifact.label, artifact.path.display());
        }
    }
}

fn write_capture_bundle(
    capture: &PngCapture,
    requested_stem: Option<&str>,
) -> Result<Vec<CaptureArtifactLink>> {
    let (width, height) = capture_dimensions(&capture.result)?;
    let views = if capture
        .result
        .get("authoritative_scanout")
        .and_then(Value::as_bool)
        == Some(true)
    {
        framebuffer_views::derive_15khz_views(&capture.png, width, height)?
    } else {
        None
    };
    let stem = capture_output_stem(requested_stem, views.is_some())?;
    let mut pending = vec![PendingCaptureArtifact {
        label: "MiSTer framebuffer raw",
        path: capture_artifact_path(&stem, "-raw.png"),
        png: capture.png.clone(),
    }];
    if let Some(views) = views {
        pending.push(PendingCaptureArtifact {
            label: "MiSTer framebuffer raw letterbox 4:3",
            path: capture_artifact_path(&stem, "-raw-letterbox-4x3.png"),
            png: views.raw_letterbox_png,
        });
        pending.push(PendingCaptureArtifact {
            label: "MiSTer framebuffer display 4:3",
            path: capture_artifact_path(&stem, "-display-4x3.png"),
            png: views.display_4x3_png,
        });
    }
    let links = pending
        .iter()
        .map(|artifact| CaptureArtifactLink {
            label: artifact.label,
            path: artifact.path.clone(),
        })
        .collect::<Vec<_>>();
    write_capture_files(&pending)?;
    Ok(links)
}

fn capture_dimensions(result: &Value) -> Result<(usize, usize)> {
    let width = usize::try_from(
        result
            .get("width")
            .and_then(Value::as_u64)
            .ok_or("agent framebuffer capture response missing width")?,
    )?;
    let height = usize::try_from(
        result
            .get("height")
            .and_then(Value::as_u64)
            .ok_or("agent framebuffer capture response missing height")?,
    )?;
    Ok((width, height))
}

fn capture_output_stem(requested: Option<&str>, has_views: bool) -> Result<PathBuf> {
    if let Some(requested) = requested {
        return normalize_capture_stem(Path::new(requested));
    }
    if io::stdout().is_terminal() {
        let desktop = PathBuf::from(env::var("HOME")?).join("Desktop");
        if !desktop.is_dir() {
            return Err(format!("Desktop directory does not exist: {}", desktop.display()).into());
        }
        let output = Command::new("date").arg("+%Y-%m-%d at %H.%M.%S").output()?;
        if !output.status.success() {
            return Err("could not determine local capture time".into());
        }
        let timestamp = String::from_utf8(output.stdout)?.trim().to_string();
        return unique_capture_stem(
            &desktop,
            &format!("MiSTer Framebuffer {timestamp}"),
            has_views,
            " ",
        );
    }

    let directory = env::temp_dir().join("mister-magik").join("captures");
    fs::create_dir_all(&directory)?;
    let directory = fs::canonicalize(directory)?;
    unique_capture_stem(
        &directory,
        &format!("mister-magik-framebuffer-{}", unix_ms_now()),
        has_views,
        "-",
    )
}

fn normalize_capture_stem(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    if path.exists() && path.is_dir() {
        return Err(format!("capture output stem is a directory: {}", path.display()).into());
    }
    let file_name = path
        .file_name()
        .ok_or("capture output stem must name a file")?;
    let stem = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        path.with_file_name(
            Path::new(file_name)
                .file_stem()
                .ok_or("capture output stem must name a file")?,
        )
    } else {
        path
    };
    if stem.file_name().is_none() {
        return Err("capture output stem must not be empty".into());
    }
    Ok(stem)
}

fn unique_capture_stem(
    directory: &Path,
    base: &str,
    has_views: bool,
    separator: &str,
) -> Result<PathBuf> {
    if !directory.is_dir() {
        return Err(format!(
            "capture output directory does not exist: {}",
            directory.display()
        )
        .into());
    }
    for suffix in 1_u64.. {
        let name = if suffix == 1 {
            base.to_string()
        } else {
            format!("{base}{separator}{suffix}")
        };
        let stem = directory.join(name);
        if capture_paths_available(&stem, has_views) {
            return Ok(stem);
        }
    }
    unreachable!("capture suffix space exhausted")
}

fn capture_artifact_path(stem: &Path, suffix: &str) -> PathBuf {
    let mut file_name = stem.file_name().unwrap_or_default().to_os_string();
    file_name.push(suffix);
    stem.with_file_name(file_name)
}

fn capture_paths_available(stem: &Path, has_views: bool) -> bool {
    let mut paths = vec![capture_artifact_path(stem, "-raw.png")];
    if has_views {
        paths.push(capture_artifact_path(stem, "-raw-letterbox-4x3.png"));
        paths.push(capture_artifact_path(stem, "-display-4x3.png"));
    }
    paths.iter().all(|path| !path.exists())
}

fn write_capture_files(artifacts: &[PendingCaptureArtifact]) -> Result<()> {
    if artifacts.iter().any(|artifact| artifact.path.exists()) {
        return Err("one or more capture output files already exist".into());
    }
    let mut created = Vec::with_capacity(artifacts.len());
    let result = (|| -> Result<()> {
        for artifact in artifacts {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&artifact.path)?;
            created.push(artifact.path.clone());
            file.write_all(&artifact.png)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        for path in created {
            let _ = fs::remove_file(path);
        }
    }
    result
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

fn validate_capture_buffer_args(args: &[String]) -> Result<()> {
    if args.is_empty() || (args.len() == 2 && args[0] == "--output" && !args[1].trim().is_empty()) {
        Ok(())
    } else {
        Err("usage: scripts/agent device capture framebuffer [--output STEM]".into())
    }
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
    })
}

fn validate_png(png: &[u8]) -> Result<()> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("agent framebuffer capture returned invalid PNG data".into());
    }
    Ok(())
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

fn opt_ms(value: Option<u128>) -> String {
    value
        .map(|milliseconds| milliseconds.to_string())
        .unwrap_or_default()
}

fn agent_reboot_wait(args: &[String]) -> Result<()> {
    let connection = ConnectionConfig::from_environment();
    let endpoint = AgentEndpoint::from_environment()?;
    agent_reboot_wait_with_config(args, &connection, &endpoint)
}

fn agent_reboot_wait_with_config(
    args: &[String],
    connection: &ConnectionConfig,
    endpoint: &AgentEndpoint,
) -> Result<()> {
    if !args.is_empty() {
        return Err("device reboot accepts only --attended".into());
    }
    let reboot_mode = RebootMode::Supervised;
    let timeout_secs = 120.0;
    let mode = reboot_mode.label();
    let issue_t = Instant::now();
    let session = connect_with(connection, 10)?;
    let reply = issue_reboot(&session, reboot_mode)?;
    let issue_ms = issue_t.elapsed().as_millis();
    println!(
        "reboot issued to {} after {issue_ms}ms: {reply}",
        connection.host()
    );
    drop(session);

    let start = Instant::now();
    let mut down_ms = None;
    while start.elapsed().as_secs_f64() < 40.0 {
        let ssh_label = tcp_probe_label_port_with(connection, 22, Duration::from_millis(100));
        let agent_label =
            tcp_probe_label_port_with(connection, AGENT_PORT, Duration::from_millis(100));
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
            let agent_probe =
                agent_request_at(endpoint, "ping", json!({}), Duration::from_millis(300));
            match agent_probe {
                Ok(_) => {
                    agent_ready_ms = Some(start.elapsed().as_millis());
                    println!("  agent ready after {}ms", opt_ms(agent_ready_ms));
                }
                Err(err) => last_note = err.to_string(),
            }
        }
        if ssh_ready_ms.is_none() {
            let ssh_probe = connect_with(connection, 2);
            match ssh_probe {
                Ok(session) => {
                    let out = exec(&session, "cat /proc/uptime", true)?;
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
                DEFAULT_LAUNCHER_ENV_REMOTE.as_str(),
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
    let agent_log = installed_layout::app_path(Layout::Development, "bootlogs/agent.log")?;
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
            "agent_persistent_log_tail": tail_remote(&sess, &agent_log, 160).map(|lines| lines.join("\n")),
            "boot_analytics_tail": tail_remote(&sess, "/tmp/mister-magik-boot-analytics.tsv", 80).map(|lines| lines.join("\n")),
        },
        "crashes": ssh_crash_reports_json(&sess),
        "catalog_failures": ssh_catalog_failure_reports_json(&sess),
        "media_diagnostics": ssh_latest_diagnostic_report(&sess, "diagnostics/media/latest.json", "updated_unix_ms"),
        "media_live": remote_read(&sess, "/tmp/mister-magik/media-diagnostics.json"),
        "catalog_progress": ssh_latest_diagnostic_report(
            &sess,
            "diagnostics/catalog/progress-latest.json",
            "updated_unix_ms",
        ),
        "latch_failure": ssh_current_latch_failure_report(&sess),
        "fpga_video_diagnostics": {
            "schema": "mister-magik-fpga-video-diagnostics-v1",
            "available": false,
            "coherent": false,
            "classification": "unclassified",
            "reason": "agent transport unavailable; raw FPGA UIO is not read over SSH",
        },
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
    write_json_member(
        out_dir,
        "fpga-video-diagnostics.json",
        bundle.get("fpga_video_diagnostics"),
    )?;

    write_json_member(
        out_dir,
        "media-diagnostics-latest.json",
        bundle.pointer("/media_diagnostics/report"),
    )?;
    write_string_pointer(
        out_dir,
        "media-diagnostics-live.json",
        bundle.get("media_live"),
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
    let crash_dir = configured_remote_path(
        "MISTER_MAGIK_APP_DIR",
        installed_layout::paths(Layout::Public).root,
    ) + "/crashes";
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
    let configured = configured_remote_path(
        "MISTER_MAGIK_APP_DIR",
        installed_layout::paths(Layout::Public).root,
    ) + "/diagnostics/catalog";
    let mut dirs = vec![
        configured,
        installed_layout::app_path(Layout::Public, "diagnostics/catalog")
            .expect("static installed path"),
        installed_layout::app_path(Layout::Development, "diagnostics/catalog")
            .expect("static installed path"),
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
    let configured = configured_remote_path(
        "MISTER_MAGIK_APP_DIR",
        installed_layout::paths(Layout::Public).root,
    );
    let Ok(public) = installed_layout::app_path(Layout::Public, relative_path) else {
        return Value::Null;
    };
    let Ok(development) = installed_layout::app_path(Layout::Development, relative_path) else {
        return Value::Null;
    };
    let mut paths = vec![format!("{configured}/{relative_path}"), public, development];
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
        configured_remote_path(
            "MISTER_MAGIK_APP_DIR",
            installed_layout::paths(Layout::Public).root,
        ),
        installed_layout::paths(Layout::Public).root.to_owned(),
        installed_layout::paths(Layout::Development).root.to_owned(),
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
    let crash_dir = configured_remote_path(
        "MISTER_MAGIK_APP_DIR",
        installed_layout::paths(Layout::Public).root,
    ) + "/crashes";
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

#[cfg(test)]
fn parse_library_db_queries(args: &[String]) -> Result<(String, Vec<String>)> {
    let mut remote_path = configured_remote_path(
        "MISTER_MAGIK_LIBRARY_DB",
        DEFAULT_REMOTE_LIBRARY_DB.as_str(),
    );
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
    let session = connect_with(connection, 2).ok()?;
    let out = exec(&session, "pidof MiSTer || echo BOOTING", true).ok()?;
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
    MenuOutput(MenuOutputProfile),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuOutputProfile {
    Crt240p60,
    Crt288p50,
    Crt480p60,
    Crt576p50,
}

impl MenuOutputProfile {
    fn settings(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Crt240p60 => ("1", "0", "0"),
            Self::Crt288p50 => ("1", "1", "0"),
            Self::Crt480p60 => ("1", "0", "1"),
            Self::Crt576p50 => ("1", "1", "1"),
        }
    }
}

fn edit_remote_ini(sess: &Session, edit: IniEdit, dry_run: bool) -> Result<()> {
    const INI: &str = "/media/fat/MiSTer.ini";
    let input = remote_read(sess, INI).ok_or("could not read /media/fat/MiSTer.ini")?;
    let edited = edit_mister_ini(&input, edit);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/media/fat/MiSTer.ini.agent-cli-new";
    remote_write(sess, tmp, edited.as_bytes())?;
    let out = exec(sess, &format!("mv {} {} && sync", sh(tmp), sh(INI)), true)?;
    if out.rc != 0 {
        return Err(format!("failed to replace {INI}: {}", out.stdout).into());
    }
    println!("MiSTer.ini edited with comment-preserving Rust mutator");
    Ok(())
}

fn edit_mister_ini(input: &str, edit: IniEdit) -> String {
    let mut document = mister_magik_ini::Document::parse(input.as_bytes())
        .expect("host-provided MiSTer.ini must be valid");

    match edit {
        IniEdit::MenuOutput(profile) => {
            let (direct_video, menu_pal, forced_scandoubler) = profile.settings();
            document.set("Menu", "direct_video", direct_video);
            document.set("Menu", "menu_pal", menu_pal);
            document.set("Menu", "forced_scandoubler", forced_scandoubler);
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

#[cfg(test)]
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
        let mut config = NativeDeviceConfig::new(connection.clone(), "device-id".into());
        config.agent = Some(AgentEndpoint::new("192.0.2.5", "token-value"));

        assert_eq!(config.connection, connection);
        assert_eq!(config.device_id, "device-id");
        assert_eq!(
            config.agent,
            Some(AgentEndpoint::new("192.0.2.5", "token-value"))
        );
    }

    use std::fs;

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
        path.push(format!("agent-cli-test-{name}-{}", unix_secs()));
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
        let wire_diagnostic = "crt_trial_status_v5 schema=5 ok=1 mode=crt-576p50 duration_ms=30001 frames=1513 flips=1513 posts=1513 drops=0 final_pending=0 final_active_matches=1 unsafe_active_writes=0 pending_writes=0 alternation_misses=0 cadence_misses=0 max_interval_us=20500 max_settle_us=18000 max_render_us=1000 max_copy_us=500 max_status_us=200 post_status_retry_frames=0 max_post_status_reads=1 post_status_transport_retry_frames=1 max_post_status_wire_attempts=2 last_buffer=1 last_sequence=1513 reason=none\n";
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
            DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str(),
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
    fn shell_quote_handles_single_quotes() {
        assert_eq!(sh("/tmp/simple"), "'/tmp/simple'");
        assert_eq!(sh("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn device_process_lock_is_nonblocking_and_released_on_drop() {
        let device = format!("test-{}", std::process::id());
        let directory = env::temp_dir().join(format!("mister-magik-lock-test-{device}"));
        let first = DeviceProcessLock::acquire_at(&directory, &device).unwrap();
        assert!(matches!(
            DeviceProcessLock::acquire_at(&directory, &device),
            Err(DeviceFailure::Busy(_))
        ));
        drop(first);
        assert!(DeviceProcessLock::acquire_at(&directory, &device).is_ok());
        fs::remove_dir_all(directory).unwrap();
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
    fn capture_buffer_paths_follow_bundle_naming() {
        let stem = Path::new("/Users/example/Desktop/MiSTer Framebuffer 2026-07-20 at 14.32.08");
        assert_eq!(
            capture_artifact_path(stem, "-raw.png"),
            Path::new("/Users/example/Desktop/MiSTer Framebuffer 2026-07-20 at 14.32.08-raw.png")
        );
        assert_eq!(
            capture_artifact_path(stem, "-display-4x3.png"),
            Path::new(
                "/Users/example/Desktop/MiSTer Framebuffer 2026-07-20 at 14.32.08-display-4x3.png"
            )
        );
    }

    #[test]
    fn capture_buffer_argument_contract_uses_the_unified_cli() {
        assert!(validate_capture_buffer_args(&[]).is_ok());
        assert_eq!(
            validate_capture_buffer_args(&["extra".to_string()])
                .unwrap_err()
                .to_string(),
            "usage: scripts/agent device capture framebuffer [--output STEM]"
        );
    }

    #[test]
    fn capture_buffer_requires_png_signature() {
        assert!(validate_png(b"\x89PNG\r\n\x1a\nfixture").is_ok());
        assert!(validate_png(b"not png").is_err());
        assert!(validate_png(&[]).is_err());
    }

    #[test]
    fn capture_buffer_allocates_collision_safe_temporary_stems() {
        let root = temp_path("capture-temporary");
        let captures = root.join("mister-magik/captures");
        fs::create_dir_all(&captures).unwrap();
        let first = unique_capture_stem(
            &captures,
            "mister-magik-framebuffer-1753012345678",
            true,
            "-",
        )
        .unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "mister-magik-framebuffer-1753012345678"
        );
        for suffix in ["-raw.png", "-raw-letterbox-4x3.png", "-display-4x3.png"] {
            fs::write(capture_artifact_path(&first, suffix), b"fixture").unwrap();
        }
        let second = unique_capture_stem(
            &captures,
            "mister-magik-framebuffer-1753012345678",
            true,
            "-",
        )
        .unwrap();
        assert_eq!(
            second.file_name().unwrap(),
            "mister-magik-framebuffer-1753012345678-2"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_output_stem_strips_only_png_extension() {
        assert_eq!(
            normalize_capture_stem(Path::new("captures/arcade.png")).unwrap(),
            env::current_dir().unwrap().join("captures/arcade")
        );
        assert_eq!(
            normalize_capture_stem(Path::new("captures/arcade.raw")).unwrap(),
            env::current_dir().unwrap().join("captures/arcade.raw")
        );
    }

    #[test]
    fn capture_bundle_writes_raw_only_for_non_crt_sources() {
        let root = temp_path("capture-bundle-raw-only");
        fs::create_dir_all(&root).unwrap();
        let stem = root.join("arcade.png");
        let capture = PngCapture {
            result: json!({
                "width": 640,
                "height": 480,
                "authoritative_scanout": false
            }),
            png: b"raw-png".to_vec(),
        };
        let links = write_capture_bundle(&capture, Some(stem.to_str().unwrap())).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].label, "MiSTer framebuffer raw");
        assert_eq!(links[0].path, root.join("arcade-raw.png"));
        assert_eq!(fs::read(&links[0].path).unwrap(), b"raw-png");
        assert!(!root.join("arcade-display-4x3.png").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_bundle_preflight_prevents_partial_writes() {
        let root = temp_path("capture-bundle-preflight");
        fs::create_dir_all(&root).unwrap();
        let stem = root.join("arcade");
        let collision = capture_artifact_path(&stem, "-display-4x3.png");
        fs::write(&collision, b"existing").unwrap();
        let artifacts = vec![
            PendingCaptureArtifact {
                label: "raw",
                path: capture_artifact_path(&stem, "-raw.png"),
                png: b"raw".to_vec(),
            },
            PendingCaptureArtifact {
                label: "display",
                path: collision.clone(),
                png: b"new".to_vec(),
            },
        ];
        assert!(write_capture_files(&artifacts).is_err());
        assert!(!capture_artifact_path(&stem, "-raw.png").exists());
        assert_eq!(fs::read(collision).unwrap(), b"existing");
        fs::remove_dir_all(root).unwrap();
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
    fn capture_buffer_allocates_collision_safe_desktop_stems() {
        let root = temp_path("capture-desktop");
        let desktop = root.join("Desktop");
        fs::create_dir_all(&desktop).unwrap();
        let first = unique_capture_stem(
            &desktop,
            "MiSTer Framebuffer 2026-07-20 at 14.32.08",
            true,
            " ",
        )
        .unwrap();
        for suffix in ["-raw.png", "-raw-letterbox-4x3.png", "-display-4x3.png"] {
            fs::write(capture_artifact_path(&first, suffix), b"fixture").unwrap();
        }
        let second = unique_capture_stem(
            &desktop,
            "MiSTer Framebuffer 2026-07-20 at 14.32.08",
            true,
            " ",
        )
        .unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "MiSTer Framebuffer 2026-07-20 at 14.32.08"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "MiSTer Framebuffer 2026-07-20 at 14.32.08 2"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_buffer_rejects_missing_desktop() {
        let desktop = temp_path("missing-desktop").join("Desktop");
        let error = unique_capture_stem(
            &desktop,
            "MiSTer Framebuffer 2026-07-20 at 14.32.08",
            true,
            " ",
        )
        .unwrap_err()
        .to_string();
        assert!(error.starts_with("capture output directory does not exist:"));
    }

    #[test]
    fn active_runtime_requires_the_exact_development_launcher_state() {
        let development = parse_active_runtime_status(Some(
            r#"{"executable_path":"/media/fat/MiSTer_MagiKDev","launcher_state":"LauncherActive"}"#,
        ));
        assert!(development.is_development_launcher());

        let public = parse_active_runtime_status(Some(
            r#"{"executable_path":"/media/fat/MiSTer_MagiK","launcher_state":"LauncherActive"}"#,
        ));
        assert!(!public.is_development_launcher());

        for status in [
            Some(
                r#"{"executable_path":"/media/fat/MiSTer_MagiKDev","launcher_state":"LauncherSuspended"}"#,
            ),
            Some(r#"{"executable_path":"unknown","launcher_state":"Unconfigured"}"#),
            Some("invalid"),
            None,
        ] {
            let active = parse_active_runtime_status(status);
            assert!(!active.is_development_launcher());
        }
        assert_eq!(
            parse_active_runtime_status(None).description(),
            "executable_path=unknown launcher_state=unknown"
        );
    }

    #[test]
    fn one_shot_recovery_clears_arming_and_refuses_known_reboot_instability() {
        let preflight = one_shot_recovery_preflight_command();
        assert!(preflight.contains("test ! -e /tmp/mister-magik/reboot-unstable"));
        assert!(preflight.contains("rm -f '/media/fat/mister-magik/launcher.env'"));
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
    fn typed_operator_commands_own_platform_and_scene_safety() {
        for layout in [Layout::Development, Layout::Public] {
            let verify = installed_platform_verify_command(layout);
            assert!(verify.contains("platform-v3.manifest"));
            assert!(verify.contains("sha256sum"));
            assert!(verify.contains("mister-magik-manager"));
            assert!(verify.contains("manager_sha256"));
            assert!(verify.contains("scanout_module_sha256"));
            assert!(verify.contains("latch_rbf_sha256"));
            assert!(verify.contains("platform verification"));
            assert!(verify.contains("hash mismatch"));
            assert!(verify.contains("manifest key is missing"));
        }
        assert!(release_arming_cleanup_command().contains("rebuild-on-next-boot"));
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
            "media_diagnostics": {"report": {"schema": "mister-magik-media-diagnostics-v1"}},
            "media_live": "{\"schema\":\"mister-magik-media-diagnostics-v1\"}",
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
            "fpga_video_diagnostics": {
                "schema": "mister-magik-fpga-video-diagnostics-v1",
                "available": true,
                "coherent": true,
                "classification": "final_black"
            },
        });

        write_diagnostics_bundle(&out, &bundle).unwrap();

        assert!(out.join("catalog-failures.json").exists());
        assert!(out.join("media-diagnostics-latest.json").exists());
        assert!(out.join("media-diagnostics-live.json").exists());
        // A pre-diagnostics device is still a valid bundle source.
        write_diagnostics_bundle(&out, &json!({})).unwrap();
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
        let fpga_video: Value =
            serde_json::from_slice(&fs::read(out.join("fpga-video-diagnostics.json")).unwrap())
                .unwrap();
        assert_eq!(fpga_video["classification"], "final_black");
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
