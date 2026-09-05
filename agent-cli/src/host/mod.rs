// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::transport::{AutomationAction, AutomationButton, DeviceFailure, Layout};
use mister_magik_platform_manifest_contract as platform_manifest_contract;
#[cfg(test)]
use quick_xml::Reader;
#[cfg(test)]
use quick_xml::events::{BytesStart, Event};
use rusqlite::backup::Backup;
#[cfg(test)]
use rusqlite::params;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use ssh2::Session;
use std::collections::{BTreeMap, BTreeSet};
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
mod latch_v5_qualification;
mod launcher_automation;
mod media;
mod platform_deploy;
mod remote;
mod transfer_check;

use agent_client::{
    AGENT_PORT, AgentEndpoint, agent_request, agent_request_at, agent_request_with_liveness,
    agent_runtime_upload_at, bootstrap_agent_with,
};
use platform_deploy::*;
use remote::{
    ConnectionConfig, ExecOutput, acknowledged_main_command, connect, connect_with,
    create_dir_command, exec, exec_failure_message, get, host, host_wait_diagnostics_with,
    launcher_restart_command, port_open_with, put, put_bytes, remote_subcommand,
    remove_files_command, shell_quote as sh, tcp_probe_label_port_with,
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
static DEVELOPMENT_ARTWORK_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Development, "assets/snes/snes-small-v1.rgb565a")
        .expect("static installed path")
});
static DEVELOPMENT_SETTINGS_ARTWORK_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Development, "assets/ui/settings-v1.rgb565a")
        .expect("static installed path")
});
static DEVELOPMENT_AGENT_REMOTE: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Development, "mister-magik-agent")
        .expect("static installed path")
});
const MAIN_STATUS_REMOTE: &str = "/tmp/mister-magik/main-status.json";
const SLINT_STATUS_REMOTE: &str = "/tmp/mister-magik/status.json";
const LATCH_FAILURE_REMOTE: &str = "/tmp/mister-magik/latch-failure.json";
const PUBLIC_MANIFEST_REMOTE: &str = mister_magik_platform_manifest_contract::PUBLIC_PATHS.manifest;
const PUBLIC_GUI_REMOTE: &str = mister_magik_platform_manifest_contract::PUBLIC_PATHS.gui;
const PUBLIC_MAIN_REMOTE: &str = mister_magik_platform_manifest_contract::PUBLIC_PATHS.main;
const DEVELOPMENT_GUI_REMOTE: &str = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.gui;
const LEGACY_RUNTIME_METADATA_PATHS: [&str; 4] = [
    "/media/fat/mister-magik/mame.sqlite3",
    "/media/fat/mister-magik/hbmame.sqlite3",
    "/media/fat/mister-magik-dev/mame.sqlite3",
    "/media/fat/mister-magik-dev/hbmame.sqlite3",
];

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

pub(crate) struct RuntimeDeliveryRequest<'a> {
    pub(crate) local: &'a Path,
    pub(crate) manifest_local: &'a Path,
    pub(crate) expected_sha256: &'a str,
    pub(crate) artwork_local: &'a Path,
    pub(crate) artwork_expected_sha256: &'a str,
    pub(crate) settings_artwork_local: &'a Path,
    pub(crate) settings_artwork_expected_sha256: &'a str,
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

    pub(crate) fn is_public_launcher(&self) -> bool {
        self.executable_path.as_deref() == Some(installed_layout::paths(Layout::Public).main)
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
            CaptureCommand, CatalogCommand, CrtCommand, DeviceCommand, DeviceFpgaCommand,
            DisplayCommand, LauncherCommand, MediaCommand, ModeCommand,
        };

        let agent = matches!(
            command,
            DeviceCommand::TransferCheck(_)
                | DeviceCommand::Capture { .. }
                | DeviceCommand::Reboot(_)
                | DeviceCommand::Logs
                | DeviceCommand::Events
                | DeviceCommand::Diagnostics(_)
                | DeviceCommand::Display {
                    command: DisplayCommand::Set(_) | DisplayCommand::Matrix(_),
                }
                | DeviceCommand::Crt { .. }
                | DeviceCommand::Launcher {
                    command: LauncherCommand::Status
                        | LauncherCommand::CaptureFirstArcade(_)
                        | LauncherCommand::LaunchReturnOnce(_)
                        | LauncherCommand::VerifyNeogeoSdram(_)
                        | LauncherCommand::CaptureCrtFontAb(_)
                        | LauncherCommand::CaptureSnesHub(_)
                        | LauncherCommand::ReturnToLauncher(_)
                }
                | DeviceCommand::Fpga {
                    command: DeviceFpgaCommand::InstallExperimental(_)
                        | DeviceFpgaCommand::InstallExperimentalAgent(_),
                }
        );
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
                DeviceCommand::Status(args) => {
                    let session = connect(10)?;
                    let status = collect_status(&session)?;
                    if args.json {
                        println!("{}", serde_json::to_string_pretty(&status)?);
                    } else {
                        print_status_summary(&status);
                    }
                    Ok(())
                }
                DeviceCommand::ArmingStatus => arming_status(),
                DeviceCommand::TransferCheck(args) => transfer_check::run(args, &prepared.config),
                DeviceCommand::Mode { command } => match command {
                    ModeCommand::Status => mode_cli(&device_strings(["status"])),
                    ModeCommand::Set(args) => mode_cli(&device_strings([args.mode.as_str()])),
                },
                DeviceCommand::Scene(args) => {
                    let mut values = device_strings([args.scene.as_str()]);
                    if let Some(seconds) = args.seconds {
                        values.push(seconds.to_string());
                    }
                    scene_cli(&values)
                }
                DeviceCommand::Display { command } => match command {
                    DisplayCommand::RouteStatus => {
                        let session = connect(10)?;
                        display_route_status(&session)
                    }
                    DisplayCommand::Set(args) => {
                        let mut values = device_strings([args.mode.as_str(), "--attended"]);
                        if args.keep {
                            values.push("--keep".into());
                        }
                        display_mode_cli(&values)
                    }
                    DisplayCommand::Matrix(args) => {
                        let mut values =
                            device_strings(["--attended", "--out", &args.out.to_string_lossy()]);
                        if args.usb_video {
                            values.push("--usb-video".into());
                        }
                        display_matrix_cli(&values)
                    }
                },
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
                DeviceCommand::Reboot(_) => agent_reboot_wait(&[]),
                DeviceCommand::Logs => agent_cli(&device_strings(["logs"])),
                DeviceCommand::Events => agent_cli(&device_strings(["timeline"])),
                DeviceCommand::Diagnostics(args) => {
                    agent_diagnostics(&device_strings(["--out", &args.out.to_string_lossy()]))
                }
                DeviceCommand::Launcher { command } => match command {
                    LauncherCommand::Status => agent_magik(&device_strings(["status"])),
                    LauncherCommand::Restart(args) => {
                        let session = connect(10)?;
                        let mut env_vars = Vec::new();
                        if let Some(experiment) = args.crt_font_experiment {
                            env_vars.push((
                                "MISTER_CRT_FONT_EXPERIMENT".into(),
                                experiment.as_str().into(),
                            ));
                        }
                        if let Some(composition) = args.crt240_composition {
                            env_vars.push((
                                "MISTER_CRT240_COMPOSITION".into(),
                                composition.as_str().into(),
                            ));
                        }
                        if !env_vars.is_empty() {
                            restart_launcher_with_one_shot_env(
                                &session,
                                LauncherRestartOptions {
                                    env_vars,
                                    timeout_secs: 45,
                                    remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
                                    ..LauncherRestartOptions::default()
                                },
                            )
                        } else {
                            launcher_restart(
                                &session,
                                &LauncherRestartOptions {
                                    clear_env: true,
                                    ..LauncherRestartOptions::default()
                                },
                            )
                        }
                    }
                    LauncherCommand::CaptureFirstArcade(args) => {
                        capture_first_arcade(&prepared.config, &args.output)
                    }
                    LauncherCommand::LaunchReturnOnce(args) => {
                        let summary_text =
                            profile_installed_launch_return_once(&prepared.config, &args.output)?;
                        let summary: Value = serde_json::from_str(&summary_text)?;
                        let artifact_status = summary
                            .get("artifact_status")
                            .and_then(Value::as_str)
                            .unwrap_or("malformed");
                        let visibility = summary
                            .get("usb_video_effective_visibility")
                            .or_else(|| summary.pointer("/usb_video/visibility"))
                            .and_then(Value::as_str)
                            .unwrap_or("malformed");
                        println!(
                            "launch-return-once artifact_status={artifact_status} \
                             magik_usb_visibility={visibility} evidence={}",
                            args.output.display()
                        );
                        validate_attended_launch_return_summary(&summary, &args.output)?;
                        thread::sleep(ATTENDED_LAUNCH_RETURN_COOLDOWN);
                        Ok(())
                    }
                    LauncherCommand::VerifyNeogeoSdram(args) => {
                        let summary =
                            profile_installed_neogeo_sdram(&prepared.config, &args.output)?;
                        println!(
                            "NeoGeo SDRAM smoke passed runs={} evidence={}",
                            summary
                                .get("runs")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len),
                            args.output.display()
                        );
                        Ok(())
                    }
                    LauncherCommand::CaptureCrtFontAb(args) => {
                        capture_crt_font_ab(&prepared.config, &args.pair, &args.output)
                    }
                    LauncherCommand::CaptureSnesHub(args) => {
                        capture_snes_hub(&prepared.config, &args.output)
                    }
                    LauncherCommand::ReturnToLauncher(_) => {
                        agent_magik(&device_strings(["return-to-launcher"]))
                    }
                },
                DeviceCommand::Catalog { command } => match command {
                    CatalogCommand::Inspect => {
                        let session = connect(10)?;
                        run_catalog_inspect(&session, &[])
                    }
                    CatalogCommand::MetadataQualification(args) => {
                        let session = connect(10)?;
                        run_runtime_metadata_qualification(&session, &args.out)
                    }
                    CatalogCommand::RomAudit(args) => {
                        let session = connect(10)?;
                        run_catalog_rom_audit(&session, &args.out)
                    }
                    CatalogCommand::NeoGeoFamilyAudit(args) => {
                        let session = connect(10)?;
                        run_catalog_neogeo_family_audit(&session, &args.out)
                    }
                    CatalogCommand::Screenshots(args) => {
                        run_catalog_screenshot_export(&args.system, &args.out)
                    }
                    CatalogCommand::ScreenshotQualification(args) => {
                        let session = connect(10)?;
                        let asset_dir = active_media_asset_dir(&session)?;
                        let status_text = remote_read(&session, MAIN_STATUS_REMOTE).ok_or(
                            "active Main status is unavailable for screenshot qualification",
                        )?;
                        let status: Value = serde_json::from_str(&status_text)?;
                        let binary = active_installed_gui_binary(&status)?;
                        media::screenshot_qualification(
                            &session,
                            &args.out_dir,
                            args.manifest_url.as_deref(),
                            &asset_dir,
                            binary,
                        )
                    }
                    CatalogCommand::Query(args) => catalog_query(&device_strings([
                        "--database",
                        &args.database,
                        "--sql",
                        &args.sql,
                    ])),
                    CatalogCommand::Cores => core_list(),
                    CatalogCommand::Purge(_) => purge_development_library_data_and_reboot(),
                },
                DeviceCommand::Media { command } => match command {
                    MediaCommand::Check(args) => {
                        let session = connect(10)?;
                        let asset_dir = active_media_asset_dir(&session)?;
                        media::media_check(&session, &device_media_args(args, &asset_dir))
                    }
                    MediaCommand::Download(args) => {
                        let session = connect(10)?;
                        let asset_dir = active_media_asset_dir(&session)?;
                        media::media_download(&session, &device_media_args(&args.media, &asset_dir))
                    }
                },
                DeviceCommand::Fpga { command } => match command {
                    DeviceFpgaCommand::InstallExperimental(args) => {
                        install_experimental_fpga_transaction(
                            &prepared.config,
                            &args.rbf,
                            &args.metadata,
                            &args.signoff_report,
                        )
                    }
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

    pub(crate) fn deliver_runtime(
        &mut self,
        delivery: RuntimeDeliveryRequest<'_>,
        timings: &mut Vec<DeliveryTimingSample>,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        deliver_runtime_transaction(
            &prepared.config,
            RuntimeDeliveryBundle {
                local: delivery.local,
                remote: DEVELOPMENT_GUI_REMOTE,
                manifest_local: delivery.manifest_local,
                manifest_remote: LOCAL_MAIN_MANIFEST_REMOTE,
                expected_sha256: delivery.expected_sha256,
                artwork_local: delivery.artwork_local,
                artwork_remote: DEVELOPMENT_ARTWORK_REMOTE.as_str(),
                artwork_expected_sha256: delivery.artwork_expected_sha256,
                settings_artwork_local: delivery.settings_artwork_local,
                settings_artwork_remote: DEVELOPMENT_SETTINGS_ARTWORK_REMOTE.as_str(),
                settings_artwork_expected_sha256: delivery.settings_artwork_expected_sha256,
            },
            timings,
        )
        .map(|_| ())
    }

    pub(crate) fn deliver_databases(
        &mut self,
        stage: &Path,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        let transaction = DatabaseDeployTransaction::validate(stage).map_err(device_failure)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        transaction
            .run(&session, &mut DeliveryTransferMetrics::default())
            .map(|_| ())
            .map_err(device_failure)
    }

    pub(crate) fn deliver_platform(
        &mut self,
        stage: &Path,
        expected_sha256: &str,
        timings: &mut Vec<DeliveryTimingSample>,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        deliver_platform_transaction(&prepared.config, stage, expected_sha256, timings).map(|_| ())
    }

    pub(crate) fn deliver_local_main(
        &mut self,
        local: &Path,
        manifest_local: &Path,
        expected_main_sha256: &str,
        expected_gui_sha256: &str,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        let mut timings = Vec::new();
        deliver_local_main_transaction(
            &prepared.config,
            local,
            manifest_local,
            expected_main_sha256,
            expected_gui_sha256,
            &mut timings,
        )
        .map(|_| ())
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

impl NativeDevice {
    pub(crate) fn verify_release_return_qualification(
        &mut self,
        certificate: &Path,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        let result = (|| -> Result<()> {
            let active =
                parse_active_runtime_status(remote_read(&session, MAIN_STATUS_REMOTE).as_deref());
            let (layout, manifest_remote) = if active.is_development_launcher() {
                (
                    crate::platform_manifest::Layout::Development,
                    LOCAL_MAIN_MANIFEST_REMOTE,
                )
            } else if active.is_public_launcher() {
                (
                    crate::platform_manifest::Layout::Public,
                    PUBLIC_MANIFEST_REMOTE,
                )
            } else {
                return Err(format!(
                    "release return evidence requires an active coherent launcher, found {}",
                    active.description()
                )
                .into());
            };
            let manifest = remote_read(&session, manifest_remote)
                .ok_or_else(|| format!("installed manifest is missing: {manifest_remote}"))?;
            crate::return_qualification::verify_aggregate_for_manifest(
                certificate,
                &manifest,
                layout,
            )?;
            Ok(())
        })();
        result.map_err(device_failure)
    }

    fn release_ssh_mutation(
        &mut self,
        operation: impl FnOnce(&NativeDeviceConfig) -> Result<()>,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_MUTATION)?;
        operation(&prepared.config).map_err(device_failure)
    }

    pub(crate) fn begin_release_qualification(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| {
            let session = connect_with(&config.connection, 10)?;
            exec_checked(
                &session,
                "release recovery preflight",
                &release_begin_command(),
            )
        })
    }

    pub(crate) fn qualify_release_runtime(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| {
            let session = connect_with(&config.connection, 10)?;
            let command = format!(
                "if pidof MiSTer_MagiKDev >/dev/null 2>&1; then {}; else {}; fi",
                delivery_health_command("dev")?,
                delivery_health_command("public")?
            );
            exec_checked(&session, "release runtime", &command)
        })
    }

    pub(crate) fn qualify_release_catalog(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| {
            let session = connect_with(&config.connection, 10)?;
            exec_checked(&session, "release catalog", &release_catalog_command())
        })
    }

    pub(crate) fn qualify_release_input_and_handoff(
        &mut self,
    ) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| {
            let session = connect_with(&config.connection, 10)?;
            exec_checked(
                &session,
                "release input and handoff",
                &release_handoff_command(),
            )
        })
    }

    pub(crate) fn qualify_release_display(&mut self) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_MUTATION)?;
        qualify_release_display_matrix_with(
            &prepared.config.connection,
            prepared.config.agent().map_err(device_failure)?,
        )
        .map(|_| ())
        .map_err(device_failure)
    }

    pub(crate) fn qualify_release_latch_v5_stress(
        &mut self,
    ) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| latch_v5_qualification::run(config).map(|_| ()))
    }

    pub(crate) fn qualify_release_recovery(&mut self) -> std::result::Result<(), DeviceFailure> {
        self.release_ssh_mutation(|config| {
            let session = connect_with(&config.connection, 10)?;
            exec_checked(&session, "release recovery", &release_recovery_command())
        })
    }

    pub(crate) fn restore_release_qualification(
        &mut self,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_MUTATION)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        exec_checked(&session, "release restore", &release_restore_command())
            .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
        issue_reboot(&session, RebootMode::Supervised)
            .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?;
        drop(session);
        if !wait_down_with(&prepared.config.connection, 40.0)
            || wait_up_with(&prepared.config.connection, 120.0)
                .map_err(|error| DeviceFailure::RecoveryRequired(error.to_string()))?
                != 0
        {
            return Err(DeviceFailure::RecoveryRequired(
                "device did not reboot after restoring release configuration".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn collect_diagnostic_facts(&mut self) -> std::result::Result<Value, DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_READ)?;
        collect_diagnostic_facts(&prepared.config)
    }

    pub(crate) fn development_fpga_activation_assessment(
        &mut self,
    ) -> std::result::Result<FpgaActivationAssessment, DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::AGENT_READ)?;
        probe_installed_fpga_activation(&prepared.config, Layout::Development)
    }

    pub(crate) fn repair_safe_device_state(&mut self) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_MUTATION)?;
        let session = connect_with(&prepared.config.connection, 10).map_err(device_failure)?;
        exec_checked(&session, "safe diagnostic repair", &safe_repair_command())
            .map_err(device_failure)
    }

    pub(crate) fn recover_with_one_shot_reboot(
        &mut self,
    ) -> std::result::Result<(), DeviceFailure> {
        let prepared = self.prepare(DeviceAccess::SSH_MUTATION)?;
        one_shot_recovery_reboot_wait(&prepared.config)
    }
}

fn collect_diagnostic_facts(
    config: &NativeDeviceConfig,
) -> std::result::Result<Value, DeviceFailure> {
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    let output = exec(&session, &diagnostic_facts_command(), false).map_err(device_failure)?;
    if let Some(message) = exec_failure_message("diagnostic facts", &output) {
        return Err(device_failure(message));
    }
    let mut facts: Value = serde_json::from_str(output.stdout.trim()).map_err(device_failure)?;
    for (path, keys) in [
        (
            MAIN_STATUS_REMOTE,
            &[
                "launcher_state",
                "crash_count",
                "last_crash_reason",
                "last_crash_report",
                "last_crash_report_id",
                "last_crash_kind",
            ][..],
        ),
        (
            SLINT_STATUS_REMOTE,
            &[
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
            ][..],
        ),
    ] {
        if let Some(status) =
            remote_read(&session, path).and_then(|text| serde_json::from_str::<Value>(&text).ok())
            && let (Some(facts), Some(status)) = (facts.as_object_mut(), status.as_object())
        {
            for key in keys {
                if let Some(value) = status.get(*key) {
                    facts.insert((*key).to_owned(), value.clone());
                }
            }
        }
    }
    if let Some(failure) = remote_read(&session, LATCH_FAILURE_REMOTE)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        && let (Some(facts), Some(failure)) = (facts.as_object_mut(), failure.as_object())
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
        match config
            .agent
            .as_ref()
            .ok_or("device agent was not prepared for diagnostic capture")
            .map_err(Into::<Box<dyn std::error::Error>>::into)
            .and_then(request_framebuffer_png_at)
        {
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
    let evidence_dir = retain_diagnostic_evidence(&session, &facts).map_err(device_failure)?;
    if let Some(facts) = facts.as_object_mut() {
        facts.insert(
            "evidence_dir".into(),
            Value::String(evidence_dir.display().to_string()),
        );
    }
    Ok(facts)
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
            DeliveryPresentState::Latch | DeliveryPresentState::CatalogMigration => {
                exec_checked(
                    &session,
                    "delivery latch health",
                    &delivery_health_command("dev")?,
                )?;
                let capture = request_framebuffer_png_at_when_latched(
                    config.agent()?,
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
            DEVELOPMENT_LAUNCHER_ENV_REMOTE.to_string(),
            "launcher.env".to_string(),
        ),
        (
            LOCAL_MAIN_MANIFEST_REMOTE.to_string(),
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
    CatalogMigration,
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
    field("screen")?;
    let effective_view = field("effective_view")?;
    field("return_screen")?;
    let present_backend = field("present_backend")?;
    let present_status = field("present_status")?;
    if field("scene")? != "launcher" {
        return Err("delivery status is not the launcher scene".into());
    }
    let input_enabled = status
        .get("input_enabled")
        .and_then(Value::as_bool)
        .ok_or("delivery status is missing input_enabled")?;
    match (present_backend, present_status) {
        ("fpga-vblank-latch-hidden", "ok") => {
            if !input_enabled {
                let catalog_migration = status
                    .get("catalog_scan_visible")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && status.get("startup_mode").and_then(Value::as_str)
                        == Some("cold_no_catalog")
                    && status.get("startup_reveal_state").and_then(Value::as_str)
                        == Some("catalog_progress_visible")
                    && status.get("frames").and_then(Value::as_u64).unwrap_or(0) > 0;
                if catalog_migration {
                    return Ok(DeliveryPresentState::CatalogMigration);
                }
                return Err("latch delivery input is not enabled outside catalog migration".into());
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryLane {
    Runtime,
    Platform,
}

impl DeliveryLane {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Platform => "platform",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryTimingStatus {
    Passed,
    Failed,
}

impl DeliveryTimingStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DeliveryTransferMetrics {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) upload_ms: u64,
    pub(crate) deploy_ms: u64,
}

impl DeliveryTransferMetrics {
    pub(crate) fn bytes_per_second(self) -> u64 {
        if self.upload_ms == 0 {
            return 0;
        }
        self.bytes.saturating_mul(1_000) / self.upload_ms
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeliveryTimingSample {
    Stage {
        lane: DeliveryLane,
        stage: &'static str,
        status: DeliveryTimingStatus,
        elapsed_ms: u64,
    },
    Transfer {
        lane: DeliveryLane,
        status: DeliveryTimingStatus,
        metrics: DeliveryTransferMetrics,
    },
    Smoke {
        lane: DeliveryLane,
        status: DeliveryTimingStatus,
        smoke_ms: u64,
    },
}

trait CoherentDeliveryActions {
    fn timing_lane(&self) -> Option<DeliveryLane> {
        None
    }

    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure>;
    fn deploy(
        &mut self,
        metrics: &mut DeliveryTransferMetrics,
    ) -> std::result::Result<(), DeviceFailure>;
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
    timings: &mut Vec<DeliveryTimingSample>,
) -> std::result::Result<String, DeviceFailure> {
    let lane = actions.timing_lane();
    timed_delivery_stage(timings, lane, "snapshot", || actions.snapshot())?;
    let delivery = (|| {
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        let mut transfer = DeliveryTransferMetrics::default();
        let deploy_started = Instant::now();
        let deploy = actions.deploy(&mut transfer);
        transfer.deploy_ms = elapsed_millis(deploy_started);
        if let Some(lane) = actions.timing_lane() {
            timings.push(DeliveryTimingSample::Transfer {
                lane,
                status: if deploy.is_ok() {
                    DeliveryTimingStatus::Passed
                } else {
                    DeliveryTimingStatus::Failed
                },
                metrics: transfer,
            });
        }
        deploy?;
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        timed_delivery_stage(timings, lane, "activate", || actions.activate())?;
        if reboots {
            timed_delivery_stage(timings, lane, "reboot", || actions.reboot())?;
        }
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        let smoke_started = Instant::now();
        let smoke = actions.smoke();
        if let Some(lane) = actions.timing_lane() {
            timings.push(DeliveryTimingSample::Smoke {
                lane,
                status: if smoke.is_ok() {
                    DeliveryTimingStatus::Passed
                } else {
                    DeliveryTimingStatus::Failed
                },
                smoke_ms: elapsed_millis(smoke_started),
            });
        }
        let detail = smoke?;
        if actions.interrupted() {
            return Err(DeviceFailure::OperationFailed(
                "delivery interrupted".into(),
            ));
        }
        Ok(detail)
    })();
    match delivery {
        Ok(detail) => timed_delivery_stage(timings, lane, "commit", || actions.commit())
            .map(|()| detail)
            .map_err(|error| {
                DeviceFailure::RecoveryRequired(format!(
                    "delivery is healthy but commit cleanup failed ({error:?})"
                ))
            }),
        Err(delivery_error) => {
            let rollback = timed_delivery_stage(timings, lane, "rollback", || actions.rollback())
                .and_then(|()| {
                    if reboots {
                        timed_delivery_stage(timings, lane, "rollback-reboot", || actions.reboot())
                    } else {
                        Ok(())
                    }
                })
                .and_then(|()| {
                    timed_delivery_stage(timings, lane, "rollback-health", || actions.health())
                });
            match rollback {
                Ok(()) => Err(delivery_error),
                Err(error) => Err(DeviceFailure::RecoveryRequired(format!(
                    "delivery failed ({delivery_error:?}); rollback failed ({error:?})"
                ))),
            }
        }
    }
}

fn timed_delivery_stage<T>(
    timings: &mut Vec<DeliveryTimingSample>,
    lane: Option<DeliveryLane>,
    stage: &'static str,
    action: impl FnOnce() -> std::result::Result<T, DeviceFailure>,
) -> std::result::Result<T, DeviceFailure> {
    let started = Instant::now();
    let result = action();
    if let Some(lane) = lane {
        timings.push(DeliveryTimingSample::Stage {
            lane,
            stage,
            status: if result.is_ok() {
                DeliveryTimingStatus::Passed
            } else {
                DeliveryTimingStatus::Failed
            },
            elapsed_ms: elapsed_millis(started),
        });
    }
    result
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
    artwork_local: &'a Path,
    artwork_remote: &'a str,
    artwork_expected_sha256: &'a str,
    settings_artwork_local: &'a Path,
    settings_artwork_remote: &'a str,
    settings_artwork_expected_sha256: &'a str,
}

impl RuntimeDeliveryActions<'_> {
    fn deploy_magik_bundle(&self, metrics: &mut DeliveryTransferMetrics) -> Result<()> {
        let total_t = Instant::now();
        let validate_t = Instant::now();
        let transaction = MagikDeployTransaction::validate_bundle(
            self.local,
            self.remote,
            self.manifest_local,
            self.manifest_remote,
            self.expected_sha256,
        )?;
        let validate_ms = validate_t.elapsed().as_millis();
        let report = transaction.run_ssh(
            self.session,
            self.config.agent()?,
            validate_ms,
            total_t,
            metrics,
        )?;
        report.print();
        Ok(())
    }
}

impl CoherentDeliveryActions for RuntimeDeliveryActions<'_> {
    fn timing_lane(&self) -> Option<DeliveryLane> {
        Some(DeliveryLane::Runtime)
    }

    fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
        exec_checked(
            self.session,
            "runtime bundle snapshot",
            &format!(
                "set -eu; cp -p {0} {0}.delivery-rollback.tmp; mv -f {0}.delivery-rollback.tmp {0}.delivery-rollback; cp -p {1} {1}.delivery-rollback.tmp; mv -f {1}.delivery-rollback.tmp {1}.delivery-rollback; mkdir -p $(dirname {2}); rm -f {2}.delivery-rollback-missing; if test -f {2}; then cp -p {2} {2}.delivery-rollback.tmp; mv -f {2}.delivery-rollback.tmp {2}.delivery-rollback; else touch {2}.delivery-rollback-missing; fi; mkdir -p $(dirname {3}); rm -f {3}.delivery-rollback-missing; if test -f {3}; then cp -p {3} {3}.delivery-rollback.tmp; mv -f {3}.delivery-rollback.tmp {3}.delivery-rollback; else touch {3}.delivery-rollback-missing; fi; sync",
                sh(self.remote),
                sh(self.manifest_remote),
                sh(self.artwork_remote),
                sh(self.settings_artwork_remote)
            ),
        )
        .map_err(device_failure)
    }

    fn deploy(
        &mut self,
        metrics: &mut DeliveryTransferMetrics,
    ) -> std::result::Result<(), DeviceFailure> {
        let artwork_upload = format!("{}.upload", self.artwork_remote);
        let artwork_bytes = fs::metadata(self.artwork_local)
            .map_err(device_failure)?
            .len();
        put_measured(
            &SshDeployRemote {
                sess: self.session,
                agent: None,
            },
            self.artwork_local,
            &artwork_upload,
            artwork_bytes,
            metrics,
        )
        .map_err(device_failure)?;
        exec_checked(
            self.session,
            "SNES artwork activation",
            &format!(
                "set -eu; test \"$(sha256sum {0} | awk '{{print $1}}')\" = {1}; mv -f {0} {2}; sync",
                sh(&artwork_upload),
                sh(self.artwork_expected_sha256),
                sh(self.artwork_remote)
            ),
        )
        .map_err(device_failure)?;
        let settings_artwork_upload = format!("{}.upload", self.settings_artwork_remote);
        let settings_artwork_bytes = fs::metadata(self.settings_artwork_local)
            .map_err(device_failure)?
            .len();
        put_measured(
            &SshDeployRemote {
                sess: self.session,
                agent: None,
            },
            self.settings_artwork_local,
            &settings_artwork_upload,
            settings_artwork_bytes,
            metrics,
        )
        .map_err(device_failure)?;
        exec_checked(
            self.session,
            "settings artwork activation",
            &format!(
                "set -eu; test \"$(sha256sum {0} | awk '{{print $1}}')\" = {1}; mv -f {0} {2}; sync",
                sh(&settings_artwork_upload),
                sh(self.settings_artwork_expected_sha256),
                sh(self.settings_artwork_remote)
            ),
        )
        .map_err(device_failure)?;
        self.deploy_magik_bundle(metrics).map_err(device_failure)
    }

    fn activate(&mut self) -> std::result::Result<(), DeviceFailure> {
        Ok(())
    }

    fn reboot(&mut self) -> std::result::Result<(), DeviceFailure> {
        delivery_reboot_wait(self.config)
    }

    fn smoke(&mut self) -> std::result::Result<String, DeviceFailure> {
        let output = smoke_development_delivery(self.config, self.expected_sha256)?;
        exec_checked(
            self.session,
            "SNES artwork smoke",
            &format!(
                "test \"$(sha256sum {0} | awk '{{print $1}}')\" = {1}",
                sh(self.artwork_remote),
                sh(self.artwork_expected_sha256)
            ),
        )
        .map_err(device_failure)?;
        exec_checked(
            self.session,
            "settings artwork smoke",
            &format!(
                "test \"$(sha256sum {0} | awk '{{print $1}}')\" = {1}",
                sh(self.settings_artwork_remote),
                sh(self.settings_artwork_expected_sha256)
            ),
        )
        .map_err(device_failure)?;
        Ok(output)
    }

    fn commit(&mut self) -> std::result::Result<(), DeviceFailure> {
        exec_checked(
            self.session,
            "runtime bundle commit",
            &format!(
                "rm -f {0}.delivery-rollback {1}.delivery-rollback {2}.delivery-rollback {2}.delivery-rollback-missing {3}.delivery-rollback {3}.delivery-rollback-missing; sync",
                sh(self.remote),
                sh(self.manifest_remote),
                sh(self.artwork_remote),
                sh(self.settings_artwork_remote)
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
                        "set -eu; test -f {0}.delivery-rollback; test -f {1}.delivery-rollback; mv -f {0}.delivery-rollback {0}; chmod 755 {0}; mv -f {1}.delivery-rollback {1}; if test -f {2}.delivery-rollback; then mv -f {2}.delivery-rollback {2}; else test -f {2}.delivery-rollback-missing; rm -f {2} {2}.delivery-rollback-missing; fi; if test -f {3}.delivery-rollback; then mv -f {3}.delivery-rollback {3}; else test -f {3}.delivery-rollback-missing; rm -f {3} {3}.delivery-rollback-missing; fi; sync",
                        sh(self.remote),
                        sh(self.manifest_remote),
                        sh(self.artwork_remote),
                        sh(self.settings_artwork_remote)
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

struct RuntimeDeliveryBundle<'a> {
    local: &'a Path,
    remote: &'a str,
    manifest_local: &'a Path,
    manifest_remote: &'a str,
    expected_sha256: &'a str,
    artwork_local: &'a Path,
    artwork_remote: &'a str,
    artwork_expected_sha256: &'a str,
    settings_artwork_local: &'a Path,
    settings_artwork_remote: &'a str,
    settings_artwork_expected_sha256: &'a str,
}

fn deliver_runtime_transaction(
    config: &NativeDeviceConfig,
    bundle: RuntimeDeliveryBundle<'_>,
    timings: &mut Vec<DeliveryTimingSample>,
) -> std::result::Result<String, DeviceFailure> {
    let RuntimeDeliveryBundle {
        local,
        remote,
        manifest_local,
        manifest_remote,
        expected_sha256,
        artwork_local,
        artwork_remote,
        artwork_expected_sha256,
        settings_artwork_local,
        settings_artwork_remote,
        settings_artwork_expected_sha256,
    } = bundle;
    require_delivery_sha256(expected_sha256)?;
    require_delivery_sha256(artwork_expected_sha256)?;
    require_delivery_sha256(settings_artwork_expected_sha256)?;
    validate_delivery_remote(remote).map_err(device_failure)?;
    validate_runtime_manifest_remote(manifest_remote).map_err(device_failure)?;
    if artwork_remote != DEVELOPMENT_ARTWORK_REMOTE.as_str() || !artwork_local.is_file() {
        return Err(DeviceFailure::ArtifactMismatch(
            "runtime delivery requires the canonical external SNES artwork".into(),
        ));
    }
    if settings_artwork_remote != DEVELOPMENT_SETTINGS_ARTWORK_REMOTE.as_str()
        || !settings_artwork_local.is_file()
    {
        return Err(DeviceFailure::ArtifactMismatch(
            "runtime delivery requires the canonical external settings artwork".into(),
        ));
    }
    let artwork_actual_sha256 = file_sha256(artwork_local.to_path_buf()).map_err(device_failure)?;
    if artwork_actual_sha256 != artwork_expected_sha256 {
        return Err(DeviceFailure::ArtifactMismatch(format!(
            "SNES artwork hash mismatch expected={artwork_expected_sha256} actual={artwork_actual_sha256}"
        )));
    }
    let settings_artwork_actual_sha256 =
        file_sha256(settings_artwork_local.to_path_buf()).map_err(device_failure)?;
    if settings_artwork_actual_sha256 != settings_artwork_expected_sha256 {
        return Err(DeviceFailure::ArtifactMismatch(format!(
            "settings artwork hash mismatch expected={settings_artwork_expected_sha256} actual={settings_artwork_actual_sha256}"
        )));
    }
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
            artwork_local,
            artwork_remote,
            artwork_expected_sha256,
            settings_artwork_local,
            settings_artwork_remote,
            settings_artwork_expected_sha256,
        },
        false,
        timings,
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
    fn timing_lane(&self) -> Option<DeliveryLane> {
        Some(DeliveryLane::Platform)
    }

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

    fn deploy(
        &mut self,
        metrics: &mut DeliveryTransferMetrics,
    ) -> std::result::Result<(), DeviceFailure> {
        self.transaction
            .run(self.session, metrics)
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
        let architecture = platform_fpga_smoke(self.config)?;
        let detail = smoke_development_delivery(self.config, self.expected_sha256)?;
        Ok(format!("{detail} fpga_architecture={architecture}"))
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
    timings: &mut Vec<DeliveryTimingSample>,
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
        timings,
    )
}

const LOCAL_MAIN_REMOTE: &str = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.main;
const LOCAL_MAIN_MANIFEST_REMOTE: &str =
    mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.manifest;

fn local_main_transaction_remote() -> String {
    installed_layout::app_path(Layout::Development, "local-main.delivery-state")
        .expect("static installed path")
}

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

    fn deploy(
        &mut self,
        metrics: &mut DeliveryTransferMetrics,
    ) -> std::result::Result<(), DeviceFailure> {
        let candidate = parse_local_main_manifest(self.manifest_local).map_err(device_failure)?;
        validate_local_main_overlay_preserves_installed(
            self.installed_manifest.as_ref().ok_or_else(|| {
                DeviceFailure::OperationFailed("local Main snapshot identity is missing".into())
            })?,
            &candidate,
        )
        .map_err(|error| DeviceFailure::ArtifactMismatch(error.to_string()))?;
        let session = self.connect()?;
        put_measured(
            &SshDeployRemote {
                sess: &session,
                agent: None,
            },
            self.local,
            &format!("{LOCAL_MAIN_REMOTE}.upload"),
            fs::metadata(self.local).map_err(device_failure)?.len(),
            metrics,
        )
        .map_err(device_failure)?;
        put_measured(
            &SshDeployRemote {
                sess: &session,
                agent: None,
            },
            self.manifest_local,
            &format!("{LOCAL_MAIN_MANIFEST_REMOTE}.upload"),
            fs::metadata(self.manifest_local)
                .map_err(device_failure)?
                .len(),
            metrics,
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
        ("gui_path", DEVELOPMENT_GUI_REMOTE),
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
    platform_manifest_contract::parse(
        text,
        platform_manifest_contract::Layout::Development,
        platform_manifest_contract::ValidationProfile::AgentStrict,
    )
    .map(platform_manifest_contract::ParsedManifest::into_values)
    .map_err(|error| format!("local Main manifest is invalid: {error}").into())
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
    platform_manifest_contract::qualification_candidate_id(fields)
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
        transaction = sh(&local_main_transaction_remote()),
    )
}

fn local_main_reconcile_script() -> String {
    format!(
        "set -eu; state=none; test ! -f {transaction} || state=$(cat {transaction}); if test \"$state\" = validated; then rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload {transaction}; sync; printf 'local-main-reconcile=validated\\n'; elif test -f {transaction}; then test -f {main}.delivery-rollback; test -f {manifest}.delivery-rollback; cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; sync; rm -f {transaction}; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; sync; printf 'local-main-reconcile=%s\\n' \"$state\"; elif test -f {main}.delivery-rollback && test -f {manifest}.delivery-rollback; then cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; sync; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; sync; printf 'local-main-reconcile=orphan\\n'; else rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; printf 'local-main-reconcile=none\\n'; fi",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(&local_main_transaction_remote()),
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
        transaction = sh(&local_main_transaction_remote()),
    )
}

fn local_main_rollback_script() -> String {
    format!(
        "set -eu; test -f {transaction}; test -f {main}.delivery-rollback; test -f {manifest}.delivery-rollback; cp -p {main}.delivery-rollback {main}; chmod 755 {main}; cp -p {manifest}.delivery-rollback {manifest}; printf 'rolled-back\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; rm -f {main}.upload {manifest}.upload; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(&local_main_transaction_remote()),
    )
}

fn local_main_cleanup_script() -> String {
    format!(
        "set -eu; test -f {transaction}; printf 'validated\\n' > {transaction}.tmp; mv -f {transaction}.tmp {transaction}; sync; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; rm -f {transaction}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(&local_main_transaction_remote()),
    )
}

fn local_main_rollback_cleanup_script() -> String {
    format!(
        "set -eu; test \"$(cat {transaction})\" = rolled-back; rm -f {main}.delivery-rollback {manifest}.delivery-rollback {main}.upload {manifest}.upload; rm -f {transaction}; sync",
        main = sh(LOCAL_MAIN_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(&local_main_transaction_remote()),
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
    timings: &mut Vec<DeliveryTimingSample>,
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
        timings,
    )
}

const EXPERIMENTAL_FPGA_RBF_REMOTE: &str =
    mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.latch_rbf;
const EXPERIMENTAL_FPGA_METADATA_REMOTE: &str =
    mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.latch_metadata;
const PATCHED_DIAGNOSTIC_ARCHITECTURE: &str = "scaler-off-domain-scheduler-terminal-v6";
const PLATFORM_V0_34_SCHEMA14_RBF_SHA256: &str =
    "ef1920500c925d35b23808792f0930954446a6030b33d3e92c0f4feccd23106e";
const FPGA_READINESS_TIMEOUT: Duration = Duration::from_secs(45);
const FPGA_READINESS_POLL: Duration = Duration::from_millis(100);
const FPGA_RELOADABLE_STREAK: usize = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FpgaCheckFailure {
    pub(crate) check: String,
    pub(crate) expected: String,
    pub(crate) actual: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FpgaActivationAssessment {
    Current {
        architecture: String,
        warning: Option<String>,
    },
    Stale {
        expected: String,
        observed: String,
        failures: Vec<FpgaCheckFailure>,
    },
    NotReady {
        expected: String,
        observed: String,
        failures: Vec<FpgaCheckFailure>,
    },
    ArtifactInvalid {
        detail: String,
    },
}

impl FpgaActivationAssessment {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Current {
                architecture,
                warning,
            } => match warning {
                Some(warning) => {
                    format!("current architecture={architecture} warning={warning}")
                }
                None => format!("current architecture={architecture}"),
            },
            Self::Stale {
                expected,
                observed,
                failures,
            }
            | Self::NotReady {
                expected,
                observed,
                failures,
            } => format!(
                "expected={expected} observed={observed} checks={}",
                render_fpga_check_failures(failures)
            ),
            Self::ArtifactInvalid { detail } => detail.clone(),
        }
    }

    fn reloadable_not_ready(&self) -> bool {
        // "unavailable" also describes normal boot before Main owns the FPGA.
        // Never turn missing evidence into a stale identity that permits reload.
        self.reloadable_fallback() || self.reloadable_same_identity_coherence()
    }

    fn reloadable_same_identity_coherence(&self) -> bool {
        matches!(
            self,
            Self::NotReady {
                expected,
                observed,
                failures,
            } if expected == observed
                && !failures.is_empty()
                && failures.iter().all(|failure| failure.check == "coherence")
        )
    }

    fn reloadable_fallback(&self) -> bool {
        matches!(
            self,
            Self::NotReady {
                observed,
                ..
            } if observed == "unverified-observer-fallback-v1"
        )
    }

    fn into_stale(self) -> Self {
        match self {
            Self::NotReady {
                expected,
                observed,
                failures,
            } => Self::Stale {
                expected,
                observed,
                failures,
            },
            assessment => assessment,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FpgaReadinessAction {
    Continue,
    Reload,
    Fail,
}

fn fpga_readiness_action(
    assessment: &FpgaActivationAssessment,
    reloadable_streak: usize,
    elapsed: Duration,
) -> FpgaReadinessAction {
    match assessment {
        FpgaActivationAssessment::Current { .. } => FpgaReadinessAction::Continue,
        FpgaActivationAssessment::Stale { .. } => FpgaReadinessAction::Reload,
        FpgaActivationAssessment::ArtifactInvalid { .. } => FpgaReadinessAction::Fail,
        FpgaActivationAssessment::NotReady { .. }
            if assessment.reloadable_fallback()
                && reloadable_streak >= FPGA_RELOADABLE_STREAK
                && elapsed >= Duration::from_millis(500) =>
        {
            FpgaReadinessAction::Reload
        }
        FpgaActivationAssessment::NotReady { .. }
            if assessment.reloadable_same_identity_coherence()
                && reloadable_streak >= FPGA_RELOADABLE_STREAK
                && elapsed >= FPGA_READINESS_TIMEOUT =>
        {
            FpgaReadinessAction::Reload
        }
        FpgaActivationAssessment::NotReady { .. } if elapsed >= FPGA_READINESS_TIMEOUT => {
            FpgaReadinessAction::Fail
        }
        FpgaActivationAssessment::NotReady { .. } => FpgaReadinessAction::Continue,
    }
}

fn render_fpga_check_failures(failures: &[FpgaCheckFailure]) -> String {
    if failures.is_empty() {
        return "none".into();
    }
    failures
        .iter()
        .map(|failure| {
            format!(
                "{} expected={} actual={}",
                failure.check, failure.expected, failure.actual
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn main_console_snapshot(session: &Session) -> String {
    let Some(raw) = remote_read(session, "/dev/vcs1") else {
        return "unavailable".into();
    };
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut tail = compact.chars().rev().take(1200).collect::<Vec<_>>();
    tail.reverse();
    tail.into_iter().collect()
}

fn experimental_fpga_transaction_remote() -> String {
    installed_layout::app_path(Layout::Development, "experimental-fpga.delivery-state")
        .expect("static installed path")
}

fn unique_field(text: &str, name: &str) -> Result<String> {
    let prefix = format!("{name}=");
    let mut values = text.lines().filter_map(|line| line.strip_prefix(&prefix));
    let value = values
        .next()
        .ok_or_else(|| format!("missing field {name}"))?;
    if value.is_empty() || values.next().is_some() {
        return Err(format!("field {name} is empty or duplicated").into());
    }
    Ok(value.to_owned())
}

fn expected_fpga_architecture(metadata: &str) -> Result<String> {
    if metadata
        .lines()
        .any(|line| line.starts_with("diagnostic_architecture="))
    {
        return unique_field(metadata, "diagnostic_architecture");
    }
    if unique_field(metadata, "rbf_sha256")? == PLATFORM_V0_34_SCHEMA14_RBF_SHA256 {
        return Ok(PATCHED_DIAGNOSTIC_ARCHITECTURE.to_owned());
    }
    Err("installed FPGA metadata does not identify the expected diagnostic architecture".into())
}

fn assess_fpga_evidence(expected: &str, evidence: &Value) -> FpgaActivationAssessment {
    let observed = evidence
        .get("diagnostic_architecture")
        .and_then(Value::as_str)
        .unwrap_or("unavailable")
        .to_owned();
    let mut failures = Vec::new();
    if observed != expected {
        failures.push(FpgaCheckFailure {
            check: "diagnostic_architecture".into(),
            expected: expected.into(),
            actual: observed.clone(),
        });
    }
    if evidence.get("schema").and_then(Value::as_str)
        != Some("mister-magik-fpga-video-diagnostics-v2")
    {
        failures.push(FpgaCheckFailure {
            check: "schema".into(),
            expected: "mister-magik-fpga-video-diagnostics-v2".into(),
            actual: evidence
                .get("schema")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
                .into(),
        });
    }
    if evidence.get("available").and_then(Value::as_bool) != Some(true) {
        failures.push(FpgaCheckFailure {
            check: "availability".into(),
            expected: "available".into(),
            actual: evidence
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
                .into(),
        });
    }
    let diagnostic_warning = (observed == expected
        && experimental_fpga_observer_fault_is_operationally_current(evidence))
    .then(|| "diagnostic_observer_self_faulted".to_owned());
    let current = observed == expected
        && (experimental_fpga_evidence_is_current(evidence) || diagnostic_warning.is_some());
    if !current {
        failures.push(FpgaCheckFailure {
            check: "coherence".into(),
            expected: "current".into(),
            actual: fpga_coherence_failure_detail(evidence),
        });
    }
    if current {
        return FpgaActivationAssessment::Current {
            architecture: expected.into(),
            warning: diagnostic_warning,
        };
    }
    let passive_fallback = observed == "unavailable"
        || observed == "unverified-observer-fallback-v1"
        || evidence
            .get("passive_observer_probe_error")
            .and_then(Value::as_str)
            .is_some();
    if observed != expected && !passive_fallback {
        FpgaActivationAssessment::Stale {
            expected: expected.into(),
            observed,
            failures,
        }
    } else {
        FpgaActivationAssessment::NotReady {
            expected: expected.into(),
            observed,
            failures,
        }
    }
}

fn fpga_coherence_failure_detail(evidence: &Value) -> String {
    let classification = evidence
        .get("classification")
        .and_then(Value::as_str)
        .unwrap_or("inconclusive");
    let coherence = evidence
        .get("coherence")
        .map(Value::to_string)
        .unwrap_or_else(|| "null".into());
    let raw_samples = evidence
        .pointer("/scaler_fetch_liveness_state/raw_samples")
        .map(Value::to_string)
        .unwrap_or_else(|| "null".into());
    format!("{classification};coherence={coherence};raw_samples={raw_samples}")
}

fn probe_installed_fpga_activation(
    config: &NativeDeviceConfig,
    layout: Layout,
) -> std::result::Result<FpgaActivationAssessment, DeviceFailure> {
    let session = connect_with(&config.connection, 10).map_err(device_failure)?;
    let metadata = match remote_read(&session, installed_layout::paths(layout).latch_metadata) {
        Some(metadata) => metadata,
        None => {
            return Ok(FpgaActivationAssessment::ArtifactInvalid {
                detail: "installed FPGA metadata is missing after activation".into(),
            });
        }
    };
    let expected = match expected_fpga_architecture(&metadata) {
        Ok(expected) => expected,
        Err(error) => {
            return Ok(FpgaActivationAssessment::ArtifactInvalid {
                detail: error.to_string(),
            });
        }
    };
    let response = agent_request_at(
        config.agent().map_err(device_failure)?,
        "diagnostics",
        json!({}),
        Duration::from_secs(5),
    )
    .map_err(device_failure)?;
    let evidence = match response.response.pointer("/result/fpga_video_diagnostics") {
        Some(evidence) => evidence,
        None => {
            return Ok(FpgaActivationAssessment::NotReady {
                expected,
                observed: "unavailable".into(),
                failures: vec![FpgaCheckFailure {
                    check: "fpga_video_diagnostics".into(),
                    expected: "present".into(),
                    actual: "missing".into(),
                }],
            });
        }
    };
    Ok(assess_fpga_evidence(&expected, evidence))
}

fn wait_for_fpga_activation(
    config: &NativeDeviceConfig,
) -> std::result::Result<FpgaActivationAssessment, DeviceFailure> {
    let started = Instant::now();
    let mut previous_reason = None;
    let mut reloadable_streak = 0;
    loop {
        let assessment = probe_installed_fpga_activation(config, Layout::Development)?;
        match &assessment {
            FpgaActivationAssessment::Current { .. }
            | FpgaActivationAssessment::Stale { .. }
            | FpgaActivationAssessment::ArtifactInvalid { .. } => return Ok(assessment),
            FpgaActivationAssessment::NotReady { .. } => {
                let reason = assessment.reason();
                if previous_reason.as_deref() == Some(reason.as_str())
                    && assessment.reloadable_not_ready()
                {
                    reloadable_streak += 1;
                } else {
                    reloadable_streak = 1;
                }
                previous_reason = Some(reason);
                let action =
                    fpga_readiness_action(&assessment, reloadable_streak, started.elapsed());
                match action {
                    FpgaReadinessAction::Reload | FpgaReadinessAction::Fail => {
                        return Ok(if action == FpgaReadinessAction::Reload {
                            assessment.into_stale()
                        } else {
                            assessment
                        });
                    }
                    FpgaReadinessAction::Continue => {}
                }
                thread::sleep(FPGA_READINESS_POLL);
            }
        }
    }
}

fn console_snapshot_for_config(config: &NativeDeviceConfig) -> String {
    connect_with(&config.connection, 10)
        .ok()
        .map(|session| main_console_snapshot(&session))
        .unwrap_or_else(|| "unavailable".into())
}

fn platform_fpga_smoke(config: &NativeDeviceConfig) -> std::result::Result<String, DeviceFailure> {
    let initial = wait_for_fpga_activation(config)?;
    let architecture = match initial {
        FpgaActivationAssessment::Current {
            architecture,
            warning,
        } => {
            if let Some(warning) = warning {
                println!("platform FPGA diagnostic warning: {warning}");
            }
            architecture
        }
        FpgaActivationAssessment::ArtifactInvalid { detail } => {
            return Err(DeviceFailure::ArtifactMismatch(detail));
        }
        assessment @ FpgaActivationAssessment::Stale { .. } => {
            let initial_reason = assessment.reason();
            let session = connect_with(&config.connection, 10).map_err(device_failure)?;
            if let Err(reload_error) = activate_installed_menu_fpga(config, &session) {
                return Err(DeviceFailure::Unhealthy(format!(
                    "FPGA activation assessment failed: {initial_reason}; bounded Main-owned reload failed: {reload_error}; main_console={}",
                    main_console_snapshot(&session)
                )));
            }
            match wait_for_fpga_activation(config)? {
                FpgaActivationAssessment::Current {
                    architecture,
                    warning,
                } => {
                    if let Some(warning) = warning {
                        println!("platform FPGA diagnostic warning after reload: {warning}");
                    }
                    architecture
                }
                FpgaActivationAssessment::ArtifactInvalid { detail } => {
                    return Err(DeviceFailure::ArtifactMismatch(format!(
                        "FPGA became artifact-invalid after reload: {detail}"
                    )));
                }
                assessment => {
                    return Err(DeviceFailure::Unhealthy(format!(
                        "FPGA activation did not become current after bounded Main-owned reload: before={initial_reason} after={}; main_console={}",
                        assessment.reason(),
                        console_snapshot_for_config(config)
                    )));
                }
            }
        }
        assessment @ FpgaActivationAssessment::NotReady { .. } => {
            return Err(DeviceFailure::Unhealthy(format!(
                "FPGA readiness timed out without a definite stale identity: {}; main_console={}",
                assessment.reason(),
                console_snapshot_for_config(config)
            )));
        }
    };
    Ok(architecture)
}

fn validate_experimental_fpga_inputs(
    rbf: &Path,
    metadata: &Path,
    signoff_report: &Path,
) -> Result<(String, String, String)> {
    if rbf.file_name().and_then(|name| name.to_str()) != Some("menu-magik-vblank-latch.rbf")
        || metadata.file_name().and_then(|name| name.to_str())
            != Some("menu-magik-vblank-latch.metadata.txt")
        || signoff_report.file_name().and_then(|name| name.to_str())
            != Some("quartus-delta-signoff.tsv")
        || rbf.parent() != metadata.parent()
        || rbf.parent().and_then(Path::parent) != signoff_report.parent()
    {
        return Err("experimental FPGA install requires one canonical local signoff set".into());
    }
    let metadata_text = fs::read_to_string(metadata)?;
    let report_text = fs::read_to_string(signoff_report)?;
    if unique_field(&metadata_text, "format")? != "mister-magik-fpga-release-v2"
        || unique_field(&metadata_text, "quartus_mode")? != "local"
        || unique_field(&metadata_text, "apply_patch")? != "1"
    {
        return Err("experimental FPGA metadata is not a patched local build".into());
    }
    let expected_rbf_sha256 = unique_field(&metadata_text, "rbf_sha256")?;
    require_delivery_sha256(&expected_rbf_sha256).map_err(|error| format!("{error:?}"))?;
    let actual_rbf_sha256 = file_sha256(rbf.to_path_buf())?;
    if actual_rbf_sha256 != expected_rbf_sha256 {
        return Err("experimental FPGA RBF hash does not match its metadata".into());
    }
    let report = report_text
        .lines()
        .find(|line| line.starts_with("quartus_delta_signoff_tsv\t"))
        .ok_or("experimental FPGA signoff report has no summary")?;
    let report_fields: BTreeMap<_, _> = report
        .split('\t')
        .skip(1)
        .filter_map(|field| field.split_once('='))
        .collect();
    let fully_valid = report_fields.get("valid") == Some(&"1")
        && report_fields.get("invalid_reason") == Some(&"ok");
    if !fully_valid {
        return Err("experimental FPGA install requires valid signoff".into());
    }
    for field in ["patched_setup_slack_min", "patched_hold_slack_min"] {
        let value = report_fields
            .get(field)
            .ok_or_else(|| format!("experimental FPGA report is missing {field}"))?
            .parse::<f64>()?;
        if value < 0.20 {
            return Err(format!("experimental FPGA {field} is below 0.20 ns").into());
        }
    }
    if report_fields.get("patched_tns_max_abs") != Some(&"0.0")
        || report_fields.get("custom_sync_seen") != Some(&"1")
        || report_fields.get("custom_sync_mtbf") != Some(&"1")
    {
        return Err("experimental FPGA timing or CDC evidence is incomplete".into());
    }
    let metadata_sha256 = file_sha256(metadata.to_path_buf())?;
    let contract = unique_field(&metadata_text, "platform_contract_sha256")?;
    require_delivery_sha256(&contract).map_err(|error| format!("{error:?}"))?;
    let menu_revision = unique_field(&metadata_text, "source_commit")?;
    require_local_main_hex("menu_revision", &menu_revision, 40)?;
    Ok((expected_rbf_sha256, metadata_sha256, menu_revision))
}

fn experimental_fpga_manifest(
    installed: &BTreeMap<String, String>,
    rbf_sha256: &str,
    metadata_sha256: &str,
    menu_revision: &str,
) -> Result<String> {
    let mut candidate = installed.clone();
    candidate.insert("latch_rbf_sha256".into(), rbf_sha256.into());
    candidate.insert("latch_metadata_sha256".into(), metadata_sha256.into());
    candidate.insert("menu_revision".into(), menu_revision.into());
    candidate.insert("qualification_candidate_id".into(), String::new());
    let identity = local_main_candidate_id(&candidate);
    candidate.insert("qualification_candidate_id".into(), identity);
    let mut output = String::new();
    for field in RUNTIME_MANIFEST_FIELDS {
        let value = candidate
            .get(*field)
            .ok_or_else(|| format!("installed Dev manifest is missing {field}"))?;
        output.push_str(field);
        output.push('=');
        output.push_str(value);
        output.push('\n');
    }
    parse_local_main_manifest_text(&output)?;
    Ok(output)
}

fn experimental_fpga_cleanup_script() -> String {
    format!(
        "rm -f {rbf}.delivery-rollback {metadata}.delivery-rollback {manifest}.delivery-rollback {rbf}.upload {metadata}.upload {manifest}.upload {transaction}; sync",
        rbf = sh(EXPERIMENTAL_FPGA_RBF_REMOTE),
        metadata = sh(EXPERIMENTAL_FPGA_METADATA_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
        transaction = sh(&experimental_fpga_transaction_remote()),
    )
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

fn experimental_fpga_observer_fault_is_operationally_current(diagnostics: &Value) -> bool {
    const RECORD_VALID: u64 = 1 << 0;
    const FIRST_STALL_VALID: u64 = 1 << 2;
    const OBSERVER_FAULT: u64 = 1 << 3;
    const LOW_FLAG_MASK: u64 = 0x0fff;

    let expected_schema = match diagnostics
        .get("diagnostic_architecture")
        .and_then(Value::as_str)
    {
        Some("scaler-off-domain-scheduler-terminal-v4") => 21,
        Some("scaler-off-domain-scheduler-terminal-v5") => 22,
        Some("scaler-off-domain-scheduler-terminal-v6") => 23,
        _ => return false,
    };

    experimental_fpga_transport_is_operational(diagnostics)
        && diagnostics.get("coherent").and_then(Value::as_bool) == Some(false)
        && diagnostics.get("classification").and_then(Value::as_str)
            == Some("scaler_fetch_liveness_evidence_inconclusive")
        && diagnostics
            .pointer("/coherence/three_samples_valid")
            .and_then(Value::as_bool)
            == Some(true)
        && diagnostics
            .pointer("/coherence/publication_coherent")
            .and_then(Value::as_bool)
            == Some(true)
        && diagnostics
            .pointer("/coherence/terminal_record_identical")
            .and_then(Value::as_bool)
            == Some(true)
        && diagnostics
            .pointer("/coherence/classification_stable")
            .and_then(Value::as_bool)
            == Some(false)
        && diagnostics
            .pointer("/scaler_fetch_liveness_state/raw_samples")
            .and_then(Value::as_array)
            .is_some_and(|samples| {
                samples.len() == 3
                    && samples[1..].iter().all(|sample| sample == &samples[0])
                    && samples[0].as_array().is_some_and(|words| {
                        words.len() == 4
                            && words[0].as_u64() == Some(expected_schema)
                            && words[1].as_u64().is_some_and(|flags| {
                                flags & LOW_FLAG_MASK & RECORD_VALID != 0
                                    && flags & LOW_FLAG_MASK & OBSERVER_FAULT != 0
                                    && flags & LOW_FLAG_MASK & FIRST_STALL_VALID == 0
                            })
                    })
            })
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

fn activate_installed_menu_fpga(config: &NativeDeviceConfig, session: &Session) -> Result<()> {
    let (expected_generation, expected_main_pid, previous_launcher_pid, _) =
        experimental_fpga_activation_status(session)?;
    exec_checked(
        session,
        "experimental FPGA Main-owned activation",
        &acknowledged_main_command(&format!("load_core {EXPERIMENTAL_FPGA_RBF_REMOTE}")),
    )?;
    wait_launcher_ready_after(
        session,
        previous_launcher_pid,
        Instant::now(),
        Duration::from_secs(45),
    )?;
    let (activated_generation, activated_main_pid, activated_launcher_pid, _) =
        experimental_fpga_activation_status(session)?;
    if activated_generation == expected_generation
        || activated_main_pid == expected_main_pid
        || activated_launcher_pid == previous_launcher_pid
    {
        return Err(format!(
            "experimental FPGA activation did not produce a new owned Dev Menu session: generation before={expected_generation} after={activated_generation}; Main pid before={expected_main_pid} after={activated_main_pid}; launcher pid before={previous_launcher_pid} after={activated_launcher_pid}"
        ).into());
    }
    exec_checked(
        session,
        "installed Dev platform verification after FPGA activation",
        &installed_platform_verify_command(Layout::Development),
    )?;
    verify_delivery_health(config).map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn verify_experimental_fpga_evidence(config: &NativeDeviceConfig) -> Result<()> {
    let diagnostics = agent_request_at(
        config.agent()?,
        "diagnostics",
        json!({}),
        Duration::from_secs(5),
    )?;
    let evidence = diagnostics
        .response
        .pointer("/result/fpga_video_diagnostics")
        .ok_or("experimental FPGA activation returned no FPGA evidence")?;
    if !experimental_fpga_evidence_is_current(evidence) {
        return Err(format!(
            "experimental FPGA activation did not expose coherent repair-only latch evidence: {evidence}"
        )
        .into());
    }
    Ok(())
}

fn install_experimental_fpga_transaction(
    config: &NativeDeviceConfig,
    rbf: &Path,
    metadata: &Path,
    signoff_report: &Path,
) -> Result<()> {
    let _signal_guard = LocalMainDeliverySignalGuard::install();
    let (rbf_sha256, metadata_sha256, menu_revision) =
        validate_experimental_fpga_inputs(rbf, metadata, signoff_report)?;
    let session = connect_with(&config.connection, 10)?;
    exec_checked(
        &session,
        "installed Dev platform verification before experimental FPGA install",
        &installed_platform_verify_command(Layout::Development),
    )?;
    let installed_text = remote_read(&session, LOCAL_MAIN_MANIFEST_REMOTE)
        .ok_or("installed Dev manifest is missing")?;
    let installed = parse_local_main_manifest_text(&installed_text)?;
    let metadata_text = fs::read_to_string(metadata)?;
    if installed["platform_contract_sha256"]
        != unique_field(&metadata_text, "platform_contract_sha256")?
        || installed["latch_protocol_version"]
            != unique_field(&metadata_text, "latch_protocol_version")?
        || installed["latch_capability_mask"]
            != unique_field(&metadata_text, "latch_capability_mask")?
    {
        return Err(
            "experimental FPGA build is incompatible with the installed Dev platform".into(),
        );
    }
    experimental_fpga_activation_status(&session)?;
    let manifest_text =
        experimental_fpga_manifest(&installed, &rbf_sha256, &metadata_sha256, &menu_revision)?;
    let manifest_path = env::temp_dir().join(format!(
        "mister-magik-experimental-fpga-{}-{}.manifest",
        std::process::id(),
        unix_ms_now()
    ));
    fs::write(&manifest_path, manifest_text)?;
    let manifest_sha256 = file_sha256(manifest_path.clone())?;
    let safety = platform_safety_script();
    let snapshot = format!(
        "set -eu; {safety}; test ! -e {transaction}; test ! -e {rbf}.delivery-rollback; test ! -e {metadata}.delivery-rollback; test ! -e {manifest}.delivery-rollback; rm -f {rbf}.upload {metadata}.upload {manifest}.upload; cp -p {rbf} {rbf}.delivery-rollback; cp -p {metadata} {metadata}.delivery-rollback; cp -p {manifest} {manifest}.delivery-rollback; printf 'snapshot\\n' > {transaction}; sync",
        transaction = sh(&experimental_fpga_transaction_remote()),
        rbf = sh(EXPERIMENTAL_FPGA_RBF_REMOTE),
        metadata = sh(EXPERIMENTAL_FPGA_METADATA_REMOTE),
        manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
    );
    exec_checked(&session, "experimental FPGA snapshot", &snapshot)?;
    let install = (|| -> Result<()> {
        put(
            &session,
            rbf,
            &format!("{EXPERIMENTAL_FPGA_RBF_REMOTE}.upload"),
        )?;
        put(
            &session,
            metadata,
            &format!("{EXPERIMENTAL_FPGA_METADATA_REMOTE}.upload"),
        )?;
        put(
            &session,
            &manifest_path,
            &format!("{LOCAL_MAIN_MANIFEST_REMOTE}.upload"),
        )?;
        if LOCAL_MAIN_DELIVERY_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("experimental FPGA install interrupted".into());
        }
        exec_checked(
            &session,
            "experimental FPGA activation",
            &format!(
                "set -eu; test \"$(sha256sum {rbf}.upload | awk '{{print $1}}')\" = {rbf_hash}; test \"$(sha256sum {metadata}.upload | awk '{{print $1}}')\" = {metadata_hash}; test \"$(sha256sum {manifest}.upload | awk '{{print $1}}')\" = {manifest_hash}; printf 'activating\\n' > {transaction}; mv -f {rbf}.upload {rbf}; mv -f {metadata}.upload {metadata}; mv -f {manifest}.upload {manifest}; sync",
                transaction = sh(&experimental_fpga_transaction_remote()),
                rbf = sh(EXPERIMENTAL_FPGA_RBF_REMOTE),
                metadata = sh(EXPERIMENTAL_FPGA_METADATA_REMOTE),
                manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
                rbf_hash = sh(&rbf_sha256),
                metadata_hash = sh(&metadata_sha256),
                manifest_hash = sh(&manifest_sha256),
            ),
        )?;
        activate_installed_menu_fpga(config, &session)?;
        verify_experimental_fpga_evidence(config)?;
        Ok(())
    })();
    let _ = fs::remove_file(&manifest_path);
    if let Err(error) = install {
        let rollback = (|| -> Result<()> {
            let rollback = connect_with(&config.connection, 10)?;
            exec_checked(
                &rollback,
                "experimental FPGA rollback",
                &format!(
                    "set -eu; test -f {rbf}.delivery-rollback; test -f {metadata}.delivery-rollback; test -f {manifest}.delivery-rollback; cp -p {rbf}.delivery-rollback {rbf}; cp -p {metadata}.delivery-rollback {metadata}; cp -p {manifest}.delivery-rollback {manifest}; printf 'rolled-back\\n' > {transaction}; sync",
                    transaction = sh(&experimental_fpga_transaction_remote()),
                    rbf = sh(EXPERIMENTAL_FPGA_RBF_REMOTE),
                    metadata = sh(EXPERIMENTAL_FPGA_METADATA_REMOTE),
                    manifest = sh(LOCAL_MAIN_MANIFEST_REMOTE),
                ),
            )?;
            if let Err(first_activation) = activate_installed_menu_fpga(config, &rollback) {
                drop(rollback);
                one_shot_recovery_reboot_wait(config).map_err(|error| {
                    format!(
                        "rollback activation failed ({first_activation}); one-shot recovery reboot failed ({error:?})"
                    )
                })?;
            }
            verify_delivery_health(config).map_err(|failure| format!("{failure:?}"))?;
            let cleanup = connect_with(&config.connection, 10)?;
            exec_checked(
                &cleanup,
                "experimental FPGA rollback cleanup",
                &experimental_fpga_cleanup_script(),
            )?;
            Ok(())
        })();
        return match rollback {
            Ok(()) => {
                Err(format!("experimental FPGA install failed ({error}); rollback=complete").into())
            }
            Err(rollback) => Err(format!(
                "experimental FPGA install failed ({error}); rollback failed ({rollback})"
            )
            .into()),
        };
    }
    let commit = connect_with(&config.connection, 10)?;
    exec_checked(
        &commit,
        "experimental FPGA commit",
        &experimental_fpga_cleanup_script(),
    )
    .map_err(|error| {
        format!(
            "experimental FPGA activation is verified but commit cleanup is incomplete; rollback was not attempted: {error}"
        )
    })?;
    Ok(())
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

fn device_media_args(args: &crate::commands::device::MediaArgs, asset_dir: &str) -> Vec<String> {
    let mut values = vec!["--asset-dir".into(), asset_dir.into()];
    if let Some(system) = &args.system {
        values.extend(["--system".into(), system.clone()]);
    }
    if let Some(url) = &args.manifest_url {
        values.extend(["--manifest-url".into(), url.clone()]);
    }
    values
}

fn active_media_asset_dir(session: &Session) -> Result<String> {
    let active = parse_active_runtime_status(remote_read(session, MAIN_STATUS_REMOTE).as_deref());
    let layout = if active.is_development_launcher() {
        Layout::Development
    } else if active.is_public_launcher() {
        Layout::Public
    } else {
        return Err(format!(
            "media operation requires an active coherent launcher, found {}",
            active.description()
        )
        .into());
    };
    installed_layout::app_path(layout, "assets")
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
static ATTENDED_OPERATION_INTERRUPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
struct DisplayMatrixEvidence {
    usb_video: bool,
}

extern "C" fn display_matrix_interrupt_handler(_: libc::c_int) {
    DISPLAY_MATRIX_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

extern "C" fn attended_operation_interrupt_handler(_: libc::c_int) {
    ATTENDED_OPERATION_INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn attended_operation_interrupted() -> bool {
    ATTENDED_OPERATION_INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

struct AttendedOperationSignalGuard([(libc::c_int, libc::sighandler_t); 3]);

impl AttendedOperationSignalGuard {
    fn install() -> Self {
        ATTENDED_OPERATION_INTERRUPTED.store(false, std::sync::atomic::Ordering::SeqCst);
        Self([libc::SIGHUP, libc::SIGINT, libc::SIGTERM].map(|signal| {
            let previous = unsafe {
                libc::signal(
                    signal,
                    attended_operation_interrupt_handler as *const () as libc::sighandler_t,
                )
            };
            (signal, previous)
        }))
    }
}

impl Drop for AttendedOperationSignalGuard {
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
        return Err("usage: scripts/agent device display set MODE --attended [--keep]".into());
    }
    let keep = args.len() == 3 && args[2] == "--keep";
    if args.len() == 3 && !keep {
        return Err("usage: scripts/agent device display set MODE --attended [--keep]".into());
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
    let (directory, capture_usb_video) = parse_display_matrix_args(args)?;
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

fn parse_display_matrix_args(args: &[String]) -> Result<(&str, bool)> {
    if args.first().map(String::as_str) != Some("--attended") {
        return Err(
            "usage: scripts/agent device display matrix --attended --out DIRECTORY [--usb-video]"
                .into(),
        );
    }
    let mut directory = None;
    let mut capture_usb_video = false;
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
            argument => {
                return Err(format!("unsupported display matrix argument: {argument}").into());
            }
        }
    }
    Ok((
        directory.ok_or("display matrix requires --out DIRECTORY")?,
        capture_usb_video,
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
    Ok((
        json!({"mode": mode.id, "status": "pass", "path": path, "usb_video": usb_video, "requested_output": mode.output.map(|(w,h)| format!("{w}x{h}")), "output_geometry": format!("{}x{}", output.0, output.1), "framebuffer_geometry": format!("{}x{}", framebuffer.0, framebuffer.1), "stride": stride, "capture_width": width, "capture_height": height, "bpp": bpp, "png_bytes": capture.png.len(), "sha256": sha256, "launcher_pid": ready.launcher_pid, "frames_before": frames_before, "frames_after": frames_after, "agent_elapsed_ms": capture.elapsed_ms, "elapsed_ms": started.elapsed().as_millis()}),
        ready.launcher_pid,
    ))
}

fn write_display_matrix_manifest(
    directory: &Path,
    original_mode: &str,
    entries: &[Value],
) -> Result<()> {
    let manifest = json!({"schema":"mister-magik-display-matrix-v3", "original_mode": original_mode, "captures": entries});
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

fn main_process_name(layout: Layout) -> &'static str {
    Path::new(installed_layout::paths(layout).main)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("schema-owned Main path has a file name")
}

fn active_installed_root_assignment() -> String {
    format!(
        "if pidof {development_main} >/dev/null 2>&1; then root={development_root}; else root={public_root}; fi",
        development_main = main_process_name(Layout::Development),
        development_root = sh(installed_layout::paths(Layout::Development).root),
        public_root = sh(installed_layout::paths(Layout::Public).root),
    )
}

fn named_installed_layout(layout: &str) -> Result<(Layout, &'static str)> {
    let layout = match layout {
        "dev" => Layout::Development,
        "public" => Layout::Public,
        _ => return Err(format!("unsupported delivery layout: {layout}").into()),
    };
    Ok((layout, main_process_name(layout)))
}

fn release_display_mode_command_for_runtime() -> String {
    format!(
        "set -eu; {active}; report=$(\"$root/mister-magik-fb\" latch-readiness-report --json); printf '%s\\n' \"$report\" | grep -Eq '\"state\"[[:space:]]*:[[:space:]]*\"ready\"'; plan=$(grep '^display-plan:' /tmp/mister-magik-slint.log | tail -n 1); before=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); idle=$(sed -n 's/.*\"idle\":[[:space:]]*\\(true\\|false\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$before\"; test -n \"$idle\"; after=$before; attempts=0; if test \"$idle\" != true; then while test \"$after\" -le \"$before\" && test \"$attempts\" -lt 10; do sleep 1; after=$(sed -n 's/.*\"frames\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/status.json); test -n \"$after\"; attempts=$((attempts+1)); done; test \"$after\" -gt \"$before\"; fi; printf 'plan\\t%s\\nframes\\t%s\\t%s\\nidle\\t%s\\nreadiness\\t%s\\n' \"$plan\" \"$before\" \"$after\" \"$idle\" \"$report\"",
        active = active_installed_root_assignment(),
    )
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

fn validate_delivery_remote(remote: &str) -> Result<()> {
    if matches!(remote, PUBLIC_GUI_REMOTE | DEVELOPMENT_GUI_REMOTE) {
        Ok(())
    } else {
        Err(format!("unsupported delivery remote: {remote}").into())
    }
}

fn validate_runtime_manifest_remote(remote: &str) -> Result<()> {
    if remote == LOCAL_MAIN_MANIFEST_REMOTE {
        Ok(())
    } else {
        Err(format!("unsupported runtime manifest remote: {remote}").into())
    }
}

fn delivery_smoke_command(layout: &str, expected_sha256: &str) -> Result<String> {
    let (layout, main) = named_installed_layout(layout)?;
    let directory = installed_layout::paths(layout).root;
    Ok(format!(
        "set -eu; smoke_check=initializing; status=/tmp/mister-magik/status.json; pid_before=; pid_after=; sequence_before=; sequence_after=; heartbeat_attempts=0; trap 'rc=$?; if test \"$rc\" -ne 0; then printf \"delivery_smoke_failure_tsv\\tcheck=%s\\trc=%s\\tpid_before=%s\\tpid_after=%s\\tsequence_before=%s\\tsequence_after=%s\\tattempts=%s\\n\" \"$smoke_check\" \"$rc\" \"$pid_before\" \"$pid_after\" \"$sequence_before\" \"$sequence_after\" \"$heartbeat_attempts\" >&2; fi' EXIT; smoke_check=artifact-sha256; test \"$(sha256sum {directory}/mister-magik-fb | awk '{{print $1}}')\" = '{expected_sha256}'; smoke_check=main-process; pidof {main} >/dev/null; smoke_check=launcher-process; pidof mister-magik-fb >/dev/null; {}; smoke_check=heartbeat-initial-pid; test -n \"$pid_before\"; smoke_check=heartbeat-initial-sequence; test -n \"$sequence_before\"; smoke_check=heartbeat-advance; while test \"$heartbeat_attempts\" -lt 10; do sleep 1; heartbeat_attempts=$((heartbeat_attempts+1)); candidate_pid=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); candidate_sequence=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); if test -z \"$candidate_pid\" || test -z \"$candidate_sequence\"; then continue; fi; pid_after=$candidate_pid; sequence_after=$candidate_sequence; if test \"$pid_after\" != \"$pid_before\"; then smoke_check=launcher-pid-stable; false; fi; if test \"$sequence_after\" -gt \"$sequence_before\"; then break; fi; done; smoke_check=heartbeat-final-pid; test -n \"$pid_after\"; smoke_check=heartbeat-final-sequence; test -n \"$sequence_after\"; smoke_check=heartbeat-advance; test \"$sequence_after\" -gt \"$sequence_before\"; smoke_check=launcher-scene; grep -Eq '\"scene\"[[:space:]]*:[[:space:]]*\"launcher\"' \"$status\"; smoke_check=effective-view; grep -Eq '\"effective_view\"[[:space:]]*:[[:space:]]*\"[^\"]+\"' \"$status\"; smoke_check=return-screen; grep -Eq '\"return_screen\"[[:space:]]*:[[:space:]]*\"[^\"]+\"' \"$status\"; smoke_check=rgb565; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; smoke_check=production-launcher-env-clear; test ! -e {public_launcher}; smoke_check=development-launcher-env-clear; test ! -e {development_launcher}; smoke_check=production-rebuild-clear; test ! -e {public_rebuild}; smoke_check=development-rebuild-clear; test ! -e {development_rebuild}; smoke_check=fault-launcher-env-clear; test ! -e /tmp/mister-magik/fs-fault-launcher.env; smoke_check=fault-session-clear; test ! -e /tmp/mister-magik/fs-fault-session; smoke_check=fault-json-clear; test ! -e /tmp/mister-magik/fs-fault.json; smoke_check=analytics-lease-clear; test ! -e /tmp/mister-magik/realtime-frame-analytics; smoke_check=screensaver-profile-clear; test ! -e /tmp/mister-magik/screensaver-profile; smoke_check=complete; trap - EXIT; printf 'delivery_smoke_tsv\\tvalid=1\\tpid=%s\\tsequence_before=%s\\tsequence_after=%s\\tattempts=%s\\n' \"$pid_after\" \"$sequence_before\" \"$sequence_after\" \"$heartbeat_attempts\"",
        launcher_heartbeat_initial_sample_command(),
        public_launcher = sh(&installed_layout::arming_paths()[0]),
        development_launcher = sh(&installed_layout::arming_paths()[1]),
        public_rebuild = sh(&installed_layout::arming_paths()[5]),
        development_rebuild = sh(&installed_layout::arming_paths()[6]),
    ))
}

fn launcher_heartbeat_initial_sample_command() -> &'static str {
    "pid_before=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_before=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\")"
}

fn launcher_heartbeat_sample_command() -> &'static str {
    "status=/tmp/mister-magik/status.json; pid_before=; sequence_before=; pid_after=; sequence_after=; if test -r \"$status\"; then pid_before=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_before=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sleep 2; if test -r \"$status\"; then pid_after=$(sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); sequence_after=$(sed -n 's/.*\"status_sequence\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p' \"$status\"); fi; fi"
}

const RELEASE_TOKEN: &str = "/tmp/mister-magik/release-qualification-session";
static RELEASE_SNAPSHOT: LazyLock<String> = LazyLock::new(|| {
    installed_layout::app_path(Layout::Public, "release-qualification-snapshot")
        .expect("static installed path")
});

fn release_rearm_token_command() -> String {
    format!(
        "mkdir -p /tmp/mister-magik; printf '%s\\n' attended-non-network-recovery-confirmed >{RELEASE_TOKEN}; test \"$(cat {RELEASE_TOKEN})\" = attended-non-network-recovery-confirmed"
    )
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

fn release_begin_command() -> String {
    let safety = platform_safety_script();
    let snapshot = format!(
        "snap={snapshot}; rm -rf \"$snap\"; mkdir -p \"$snap\"; if test -e /media/fat/MiSTer.ini; then cp -a /media/fat/MiSTer.ini \"$snap/MiSTer.ini\"; fi; printf '%s\\n' attended-non-network-recovery-confirmed >{RELEASE_TOKEN}; test -s {RELEASE_TOKEN}",
        snapshot = sh(RELEASE_SNAPSHOT.as_str()),
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
        "set -eu; test -s {RELEASE_TOKEN}; {active}; report=$(\"$root/mister-magik-fb\" catalog-inspect); printf '%s\\n' \"$report\" | grep -Eq 'catalog_summary_tsv[[:space:]]+valid=1'",
        active = active_installed_root_assignment(),
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
        "set -eu; test -s {RELEASE_TOKEN}; {active}; bin=\"$root/mister-magik-fb\"; test -x \"$bin\"; report=$(\"$bin\" latch-readiness-report --json); plan=$(grep '^display-plan:' /tmp/mister-magik-slint.log | tail -n 1 || true); latch=$(\"$bin\" fpga-latch-report); bpp=$(cat /sys/class/graphics/fb0/bits_per_pixel); printf 'release_display_readiness_json\\t%s\\n' \"$report\"; printf 'release_display_plan\\t%s\\n' \"$plan\"; printf 'release_display_latch\\t%s\\n' \"$latch\"; printf 'release_display_bpp\\t%s\\n' \"$bpp\"; printf '%s\\n' \"$report\" | grep -Eq '\"state\":\"ready\"'; printf '%s\\n' \"$report\" | grep -Eq '\"scanout_abi_version\":3'; printf '%s\\n' \"$report\" | grep -Eq '\"scanout_slot_capacity_bytes\":2101248'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_width\":1366'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_height\":768'; printf '%s\\n' \"$report\" | grep -Eq '\"latch_max_stride_bytes\":2736'; printf '%s\\n' \"$plan\" | grep -Eq '^display-plan: .*output={output} .*fb={framebuffer} '; printf '%s\\n' \"$latch\" | grep -q 'supported=1'; printf '%s\\n' \"$latch\" | grep -q 'drop_count=0'; test \"$bpp\" = 16; printf 'display_qualification_tsv\\tlabel={label}\\tvideo_mode={video_mode}\\toutput={output}\\tfb={framebuffer}\\tstride={stride}\\n'",
        active = active_installed_root_assignment(),
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
        "snap={snapshot}; {}; if test -s \"$snap/MiSTer.ini\"; then cp -a \"$snap/MiSTer.ini\" /media/fat/MiSTer.ini; fi; rm -f {RELEASE_TOKEN}; rm -rf \"$snap\"",
        release_arming_cleanup_command(),
        snapshot = sh(RELEASE_SNAPSHOT.as_str()),
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
    let agent_token = installed_layout::app_path(Layout::Development, "agent.token")
        .expect("static installed path");
    let arming_paths = installed_layout::arming_paths()
        .iter()
        .map(|path| sh(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "set -eu; main=false; launcher=false; agent=false; credentials=false; scanout=false; firmware=false; latch=false; unstable=false; temporary=false; launcher_heartbeat_advancing=false; {{ pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }} && main=true; pidof mister-magik-fb >/dev/null 2>&1 && launcher=true; pidof mister-magik-agent >/dev/null 2>&1 && agent=true; test -s {agent_token} && credentials=true; {{ grep -q '^mister_magik_scanout_slots ' /proc/modules 2>/dev/null && test -c /dev/mister-magik-scanout-slots; }} && scanout=true; \"$scanout\" && firmware=true; {active}; if test -x \"$root/mister-magik-fb\"; then latch_report=$(\"$root/mister-magik-fb\" latch-readiness-report 2>/dev/null || true); printf '%s\\n' \"$latch_report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready' && latch=true; fi; {}; if test -n \"$pid_before\" && test \"$pid_before\" = \"$pid_after\" && test -n \"$sequence_before\" && test -n \"$sequence_after\" && test \"$sequence_after\" -gt \"$sequence_before\"; then launcher_heartbeat_advancing=true; fi; test -e /tmp/mister-magik/reboot-unstable && unstable=true; arming=0; for path in {arming_paths}; do test ! -e \"$path\" || arming=$((arming + 1)); done; for path in /tmp/mister-magik/agent-benchmark.tsv /tmp/mister-magik/agent-benchmark-warmup.tsv /tmp/mister-magik/agent-cold-benchmark.out /tmp/mister-magik/stale-launcher-return-state.json /tmp/mister-magik/realtime-frame-analytics /tmp/mister-magik/screensaver-profile; do test ! -e \"$path\" || temporary=true; done; printf '{{\"main_running\":%s,\"launcher_running\":%s,\"agent_running\":%s,\"credentials_ready\":%s,\"firmware_compatible\":%s,\"scanout_ready\":%s,\"latch_ready\":%s,\"reboot_unstable\":%s,\"arming_files\":%s,\"temporary_state\":%s,\"launcher_heartbeat_advancing\":%s}}\\n' \"$main\" \"$launcher\" \"$agent\" \"$credentials\" \"$firmware\" \"$scanout\" \"$latch\" \"$unstable\" \"$arming\" \"$temporary\" \"$launcher_heartbeat_advancing\"",
        launcher_heartbeat_sample_command(),
        agent_token = sh(&agent_token),
        active = active_installed_root_assignment(),
        arming_paths = arming_paths,
    )
}

fn is_safe_crash_report_path(path: &str) -> bool {
    [
        format!(
            "{}/crashes/report-",
            installed_layout::paths(Layout::Public).root
        ),
        format!(
            "{}/crashes/report-",
            installed_layout::paths(Layout::Development).root
        ),
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
        return Err("usage: scripts/agent device mode <status|set MODE --attended>".into());
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
            "usage: scripts/agent device scene <launcher|controller-test|tear-pattern|video-playback|crt-trial> --attended [--seconds N]"
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
            "set -eu; test -x {gui}; {gui} ui {scene} {seconds} >/tmp/mister-magik-{scene}.log 2>&1",
            gui = sh(DEVELOPMENT_GUI_REMOTE),
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

fn modal_input_action(
    config: &NativeDeviceConfig,
    nonce: &str,
    action: AutomationAction,
) -> Result<u64> {
    let detail = launcher_automation::send_action(config, nonce, &action)?;
    let value: Value = serde_json::from_str(&detail)?;
    let sequence = value
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("modal input action has no sequence")?;
    launcher_automation::await_presented(config, nonce, sequence, 3_000)?;
    Ok(sequence)
}

fn modal_semantic<'a>(snapshot: &'a Value, field: &str) -> Option<&'a Value> {
    snapshot.pointer(&format!("/semantic/{field}"))
}

const LAUNCH_RETURN_CYCLES: usize = 2;
const LAUNCH_RETURN_ONCE_GAME: &str = "/media/fat/_Arcade/1943 Kai Midway Kaisen (Japan).mra";
const ATTENDED_LAUNCH_RETURN_COOLDOWN: Duration = Duration::from_secs(5);
const ATTENDED_LAUNCH_RETURN_GAME_DWELL: Duration = Duration::from_secs(2);
const LAUNCH_RETURN_PHYSICAL_CONFIRMATIONS: usize = 2;
const LAUNCH_RETURN_PHYSICAL_CONFIRMATION_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
struct LaunchReturnUsbConfirmation {
    visibility: crate::capture::CaptureVisibility,
    temporal_luma_delta_permille: u16,
}

fn launch_return_effective_usb_visibility(
    primary: crate::capture::CaptureVisibility,
    confirmations: &[LaunchReturnUsbConfirmation],
) -> Option<crate::capture::CaptureVisibility> {
    use crate::capture::CaptureVisibility::{Black, Corrupted, SignalLost, Visible};

    if matches!(primary, Black | SignalLost) {
        return Some(primary);
    }
    if confirmations.len() != LAUNCH_RETURN_PHYSICAL_CONFIRMATIONS {
        return None;
    }
    if confirmations
        .iter()
        .any(|confirmation| confirmation.visibility == SignalLost)
    {
        return Some(SignalLost);
    }
    if confirmations
        .iter()
        .any(|confirmation| confirmation.visibility == Black)
    {
        return Some(Black);
    }
    if primary == Corrupted
        || confirmations
            .iter()
            .any(|confirmation| confirmation.visibility == Corrupted)
        || confirmations.iter().any(|confirmation| {
            confirmation.temporal_luma_delta_permille
                >= crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE
        })
    {
        return Some(Corrupted);
    }
    debug_assert!(
        primary == Visible
            && confirmations
                .iter()
                .all(|confirmation| confirmation.visibility == Visible)
    );
    Some(Visible)
}

fn validate_attended_launch_return_summary(summary: &Value, output_dir: &Path) -> Result<()> {
    if summary.get("schema").and_then(Value::as_str) != Some("mister-magik-launch-return-once-v2") {
        return Err("attended launch-return evidence has the wrong schema".into());
    }

    let semantic = summary
        .pointer("/restored_selection/semantic")
        .and_then(Value::as_object)
        .ok_or("attended launch-return evidence has no settled MagiK semantic state")?;
    let settled_magik = semantic.get("effective_view").and_then(Value::as_str) == Some("arcade")
        && semantic.get("return_screen").and_then(Value::as_str) == Some("arcade")
        && semantic.get("selected_game_id").and_then(Value::as_str)
            == Some(LAUNCH_RETURN_ONCE_GAME)
        && semantic.get("launch_state").and_then(Value::as_str) == Some("idle")
        && semantic.get("input_enabled").and_then(Value::as_bool) == Some(true)
        && semantic
            .get("navigation_transition_active")
            .and_then(Value::as_bool)
            == Some(false);
    if !settled_magik {
        return Err(
            "attended launch-return evidence was not captured from settled post-return MagiK"
                .into(),
        );
    }

    let artifact_status = summary
        .get("artifact_status")
        .and_then(Value::as_str)
        .ok_or("attended launch-return evidence has no artifact status")?;
    let physical_visible = summary
        .get("physical_video_visible")
        .and_then(Value::as_bool)
        .ok_or("attended launch-return evidence has no physical visibility result")?;
    let usb_visibility = summary
        .get("usb_video_effective_visibility")
        .or_else(|| summary.pointer("/usb_video/visibility"))
        .and_then(Value::as_str)
        .ok_or("attended launch-return evidence has no USB-video classification")?;
    let raw_primary_visibility = summary
        .pointer("/usb_video/visibility")
        .and_then(Value::as_str);
    let raw_confirmations = summary
        .pointer("/usb_video_return_confirmation/captures")
        .and_then(Value::as_array);
    let temporal_contract_valid = summary
        .pointer("/usb_video_return_confirmation/schema")
        .and_then(Value::as_str)
        == Some("mister-magik-return-physical-confirmation-v2")
        && summary
            .pointer("/usb_video_return_confirmation/temporal_luma_grid")
            .and_then(Value::as_str)
            == Some(crate::capture::TEMPORAL_LUMA_GRID_ID)
        && summary
            .pointer("/usb_video_return_confirmation/temporal_luma_corruption_threshold_permille")
            .and_then(Value::as_u64)
            == Some(u64::from(crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE));
    let all_raw_samples_visible = temporal_contract_valid
        && raw_primary_visibility == Some("visible")
        && raw_confirmations.is_some_and(|captures| {
            captures.len() == LAUNCH_RETURN_PHYSICAL_CONFIRMATIONS
                && captures.iter().all(|capture| {
                    capture
                        .pointer("/capture/visibility")
                        .and_then(Value::as_str)
                        == Some("visible")
                        && capture
                            .get("temporal_luma_delta_permille")
                            .and_then(Value::as_u64)
                            .is_some_and(|delta| {
                                delta < u64::from(crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE)
                            })
                })
        });

    match (artifact_status, physical_visible, usb_visibility) {
        ("passed", true, "visible") if all_raw_samples_visible => Ok(()),
        ("passed", true, "visible") => Err(format!(
            "attended launch-return evidence promoted a non-visible or incomplete raw physical \
             sample set; evidence={}",
            output_dir.display()
        )
        .into()),
        ("failed", false, visibility) if visibility != "visible" => Err(format!(
            "post-return MagiK physical video failed closed: visibility={visibility}; evidence={}",
            output_dir.display()
        )
        .into()),
        _ => Err(format!(
            "attended launch-return evidence is internally inconsistent: \
             artifact_status={artifact_status} physical_visible={physical_visible} \
             usb_visibility={usb_visibility}"
        )
        .into()),
    }
}
const LAUNCH_RETURN_ONCE_STEP_DEADLINE_MS: u64 = 2_000;

fn launch_return_once_action(
    config: &NativeDeviceConfig,
    nonce: &str,
    button: AutomationButton,
) -> Result<u64> {
    let detail = launcher_automation::send_action(config, nonce, &AutomationAction::Tap(button))?;
    let value: Value = serde_json::from_str(&detail)?;
    let sequence = value
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("launch-return-once action has no sequence")?;
    launcher_automation::await_presented(config, nonce, sequence, 3_000)?;
    Ok(sequence)
}

fn launch_return_once_hold_action(
    config: &NativeDeviceConfig,
    nonce: &str,
    button: AutomationButton,
) -> Result<u64> {
    let detail = launcher_automation::send_action(
        config,
        nonce,
        &AutomationAction::Hold {
            button,
            duration_ms: 120,
        },
    )?;
    let value: Value = serde_json::from_str(&detail)?;
    let sequence = value
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("launch-return-once hold action has no sequence")?;
    launcher_automation::await_presented(config, nonce, sequence, 3_000)?;
    Ok(sequence)
}

fn launch_return_once_next_game(
    config: &NativeDeviceConfig,
    nonce: &str,
    previous: &str,
) -> Result<Value> {
    launch_return_once_hold_until_selection_changes(
        config,
        nonce,
        AutomationButton::Down,
        "selected_game_id",
        previous,
        "next Arcade game",
    )
}

fn launch_return_once_hold_until_selection_changes(
    config: &NativeDeviceConfig,
    nonce: &str,
    button: AutomationButton,
    semantic_field: &str,
    previous: &str,
    label: &str,
) -> Result<Value> {
    launcher_automation::send_action(
        config,
        nonce,
        &AutomationAction::Hold {
            button,
            duration_ms: LAUNCH_RETURN_ONCE_STEP_DEADLINE_MS,
        },
    )?;
    let changed = launch_return_once_wait(
        config,
        nonce,
        |snapshot| {
            modal_semantic(snapshot, semantic_field).and_then(Value::as_str) != Some(previous)
        },
        label,
    );
    let released = launcher_automation::send_action(config, nonce, &AutomationAction::ReleaseAll);
    match (changed, released) {
        (Ok(snapshot), Ok(_)) => Ok(snapshot),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release)) => Err(format!("{error}; release failed: {release}").into()),
    }
}

fn launch_return_once_initial_env() -> Vec<(String, String)> {
    vec![
        ("MISTER_CATALOG_REFRESH".into(), "off".into()),
        ("MISTER_ARCADE_SELECTED_INDEX".into(), "0".into()),
    ]
}

fn launch_return_once_wait(
    config: &NativeDeviceConfig,
    nonce: &str,
    predicate: impl Fn(&Value) -> bool,
    label: &str,
) -> Result<Value> {
    let started = Instant::now();
    let timeout = Duration::from_secs(10);
    loop {
        let snapshot = launcher_automation::snapshot(config, nonce)?;
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "launch-return-once timed out waiting for {label}; final snapshot={snapshot}"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn launch_return_once_select_menu_item(
    config: &NativeDeviceConfig,
    nonce: &str,
    expected_item_id: &str,
) -> Result<()> {
    let mut state = launch_return_once_wait(
        config,
        nonce,
        |snapshot| {
            modal_semantic(snapshot, "effective_view").and_then(Value::as_str) == Some("home")
        },
        "Home",
    )?;
    let count = modal_semantic(&state, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("launch-return-once Home has no selected count")?;
    let mut move_left = modal_semantic(&state, "selected_index")
        .and_then(Value::as_u64)
        .is_some_and(|index| index > 0);
    for _ in 0..count.saturating_mul(2) {
        if modal_semantic(&state, "selected_item_id").and_then(Value::as_str)
            == Some(expected_item_id)
        {
            return Ok(());
        }
        let index = modal_semantic(&state, "selected_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if (move_left && index == 0) || (!move_left && index.saturating_add(1) >= count) {
            move_left = !move_left;
        }
        let previous = modal_semantic(&state, "selected_item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        state = launch_return_once_hold_until_selection_changes(
            config,
            nonce,
            if move_left {
                AutomationButton::Left
            } else {
                AutomationButton::Right
            },
            "selected_item_id",
            &previous,
            "Home selection change",
        )?;
    }
    Err(format!("launch-return-once menu has no {expected_item_id} item").into())
}

fn launch_return_once_select_game(config: &NativeDeviceConfig, nonce: &str) -> Result<Value> {
    let mut state = launch_return_once_wait(
        config,
        nonce,
        |snapshot| {
            modal_semantic(snapshot, "effective_view").and_then(Value::as_str) == Some("arcade")
                && modal_semantic(snapshot, "selected_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
        },
        "populated Arcade view",
    )?;
    let count = modal_semantic(&state, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("launch-return-once Arcade has no selected count")?;
    for index in 0..count {
        if modal_semantic(&state, "selected_game_id").and_then(Value::as_str)
            == Some(LAUNCH_RETURN_ONCE_GAME)
        {
            return Ok(state);
        }
        if index + 1 == count {
            break;
        }
        let previous = modal_semantic(&state, "selected_game_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        state = launch_return_once_next_game(config, nonce, &previous)?;
    }
    Err(format!("launch-return-once cannot find {LAUNCH_RETURN_ONCE_GAME}").into())
}

fn launch_return_once_validate_restored_selection(
    pre_launch: &Value,
    restored: &Value,
) -> Result<()> {
    for field in ["active_collection_id", "selected_game_id"] {
        let expected = modal_semantic(pre_launch, field).and_then(Value::as_str);
        let actual = modal_semantic(restored, field).and_then(Value::as_str);
        if expected.is_none() || actual != expected {
            return Err(format!(
                "launch-return-once did not restore {field}: expected={} actual={}",
                expected.unwrap_or("missing"),
                actual.unwrap_or("missing")
            )
            .into());
        }
    }
    let expected_index = modal_semantic(pre_launch, "selected_index").and_then(Value::as_u64);
    let actual_index = modal_semantic(restored, "selected_index").and_then(Value::as_u64);
    if expected_index.is_none() || actual_index != expected_index {
        return Err(format!(
            "launch-return-once did not restore selected_index: expected={} actual={}",
            expected_index.map_or_else(|| "missing".to_string(), |value| value.to_string()),
            actual_index.map_or_else(|| "missing".to_string(), |value| value.to_string())
        )
        .into());
    }
    for (field, expected) in [
        ("effective_view", "arcade"),
        ("return_screen", "arcade"),
        ("launch_state", "idle"),
    ] {
        let actual = modal_semantic(restored, field).and_then(Value::as_str);
        if actual != Some(expected) {
            return Err(format!(
                "launch-return-once restored the wrong view: {field} expected={expected} actual={}",
                actual.unwrap_or("missing")
            )
            .into());
        }
    }
    Ok(())
}

const NEOGEO_SDRAM_GAME_DWELL: Duration = Duration::from_secs(20);
const NEOGEO_HIGH_MEMORY_SETNAMES: &[&str] = &[
    "mslug3",
    "mslug5",
    "kof2003",
    "svc",
    "garou",
    "samsho5",
    "samsho5sp",
];
const NEOGEO_CONTROL_SETNAMES: &[&str] = &["mslug", "kof98", "samsho2"];

#[derive(Clone, Debug)]
struct NeoGeoSmokeTarget {
    role: &'static str,
    game_id: String,
    setname: String,
    selected_index: u64,
}

fn neogeo_setname(game_id: &str) -> Option<String> {
    let lower = game_id.to_ascii_lowercase();
    if let Some(end) = lower.rfind(").neo") {
        let start = lower[..end].rfind('(')? + 1;
        let setname = lower[start..end].trim();
        if !setname.is_empty() {
            return Some(setname.to_owned());
        }
    }
    let filename = lower.rsplit('/').next()?;
    filename
        .strip_suffix(".neo")
        .or_else(|| filename.strip_suffix(".mgl"))
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
}

fn neogeo_smoke_matrix(entries: &[(String, u64)]) -> Result<Vec<NeoGeoSmokeTarget>> {
    let structured: BTreeMap<String, (String, u64)> = entries
        .iter()
        .filter(|(game_id, _)| game_id.starts_with("magik-plan:"))
        .filter_map(|(game_id, index)| {
            neogeo_setname(game_id).map(|setname| (setname, (game_id.clone(), *index)))
        })
        .collect();
    let target = |role, setname: &str| {
        structured
            .get(setname)
            .map(|(game_id, selected_index)| NeoGeoSmokeTarget {
                role,
                game_id: game_id.clone(),
                setname: setname.to_owned(),
                selected_index: *selected_index,
            })
    };
    let mandatory = target("mandatory-high-memory", "mslug3")
        .ok_or("NeoGeo smoke requires the structured Metal Slug 3 (mslug3) launch")?;
    let additional = NEOGEO_HIGH_MEMORY_SETNAMES
        .iter()
        .copied()
        .filter(|setname| *setname != "mslug3")
        .find_map(|setname| target("additional-high-memory", setname))
        .ok_or("NeoGeo smoke requires a second installed high-memory structured launch")?;
    let control = NEOGEO_CONTROL_SETNAMES
        .iter()
        .copied()
        .find_map(|setname| target("control", setname))
        .ok_or("NeoGeo smoke requires an installed low-memory structured control")?;
    let (mgl, mgl_index) = entries
        .iter()
        .find(|(game_id, _)| game_id.to_ascii_lowercase().ends_with(".mgl"))
        .ok_or("NeoGeo smoke requires a real .mgl entry in the NeoGeo collection")?;
    let direct_mgl = NeoGeoSmokeTarget {
        role: "direct-mgl",
        game_id: mgl.clone(),
        setname: neogeo_setname(mgl).unwrap_or_else(|| "mgl".to_string()),
        selected_index: *mgl_index,
    };
    Ok(vec![mandatory, additional, control, direct_mgl])
}

fn launch_return_once_select_game_index(
    config: &NativeDeviceConfig,
    nonce: &str,
    target: u64,
) -> Result<Value> {
    let mut state = launch_return_once_wait(
        config,
        nonce,
        |snapshot| {
            modal_semantic(snapshot, "effective_view").and_then(Value::as_str) == Some("arcade")
                && modal_semantic(snapshot, "selected_count")
                    .and_then(Value::as_u64)
                    .is_some_and(|count| target < count)
        },
        "populated NeoGeo view",
    )?;
    loop {
        let current = modal_semantic(&state, "selected_index")
            .and_then(Value::as_u64)
            .ok_or("NeoGeo view has no selected index")?;
        if current == target {
            return Ok(state);
        }
        let previous = modal_semantic(&state, "selected_game_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        state = launch_return_once_hold_until_selection_changes(
            config,
            nonce,
            if current < target {
                AutomationButton::Down
            } else {
                AutomationButton::Up
            },
            "selected_game_id",
            &previous,
            "NeoGeo selection change",
        )?;
    }
}

fn launch_return_once_open_neogeo(config: &NativeDeviceConfig, nonce: &str) -> Result<Value> {
    launch_return_once_action(config, nonce, AutomationButton::Home)?;
    for item in ["menu:consoles", "menu:snk-neogeo"] {
        launch_return_once_select_menu_item(config, nonce, item)?;
        launch_return_once_action(config, nonce, AutomationButton::A)?;
    }
    launch_return_once_select_menu_item(config, nonce, "neogeo")?;
    launch_return_once_hold_action(config, nonce, AutomationButton::A)?;
    launch_return_once_wait(
        config,
        nonce,
        |snapshot| {
            modal_semantic(snapshot, "effective_view").and_then(Value::as_str) == Some("arcade")
                && modal_semantic(snapshot, "active_collection_id").and_then(Value::as_str)
                    == Some("neogeo")
                && modal_semantic(snapshot, "selected_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
        },
        "populated NeoGeo collection",
    )
}

fn collect_neogeo_entries(
    config: &NativeDeviceConfig,
    nonce: &str,
    initial: Value,
) -> Result<Vec<(String, u64)>> {
    let count = modal_semantic(&initial, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("NeoGeo collection has no selected count")?;
    let mut state = launch_return_once_select_game_index(config, nonce, 0)?;
    let mut entries = Vec::with_capacity(count.try_into().unwrap_or(0));
    for index in 0..count {
        let game_id = modal_semantic(&state, "selected_game_id")
            .and_then(Value::as_str)
            .ok_or("NeoGeo entry has no selected game id")?;
        entries.push((game_id.to_owned(), index));
        if index + 1 < count {
            state = launch_return_once_next_game(config, nonce, game_id)?;
        }
    }
    Ok(entries)
}

fn confirm_neogeo_game(target: &NeoGeoSmokeTarget) -> Result<Value> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("NeoGeo SDRAM smoke requires an interactive terminal".into());
    }
    let token = target.setname.to_ascii_uppercase();
    eprintln!(
        "Observe {} now. Confirm the title/attract graphics are correct and no memory warning is visible by typing {token}:",
        target.game_id
    );
    io::stderr().flush()?;
    let mut acknowledgement = String::new();
    io::stdin().read_line(&mut acknowledgement)?;
    if acknowledgement.trim().to_ascii_uppercase() != token {
        return Err(format!("NeoGeo observation was not confirmed with {token}").into());
    }
    Ok(json!({
        "confirmation": token,
        "no_memory_warning": true,
        "title_and_attract_graphics_correct": true,
    }))
}

fn validate_neogeo_sdram_events(events: &str, first_new_line: usize) -> Result<Value> {
    let parsed: Vec<Value> = events
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let (handoff_index, handoff) = parsed
        .iter()
        .enumerate()
        .skip(first_new_line)
        .find(|(_, event)| {
            matches!(
                event.get("event").and_then(Value::as_str),
                Some("handoff_launch" | "handoff_launch_plan")
            )
        })
        .ok_or("NeoGeo run produced no new Main handoff event")?;
    let (ready_index, ready) = parsed[..handoff_index]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.get("event").and_then(Value::as_str) == Some("sdram_config_ready"))
        .ok_or("NeoGeo handoff has no preceding SDRAM-ready event")?;
    let detail = ready.get("detail").and_then(Value::as_str).unwrap_or("");
    if !detail.contains("size_code=3") {
        return Err(format!("NeoGeo handoff SDRAM size is not 128 MiB: {detail}").into());
    }
    if parsed[ready_index..handoff_index]
        .iter()
        .any(|event| event.get("event").and_then(Value::as_str) == Some("sdram_config_unavailable"))
    {
        return Err("NeoGeo handoff followed an SDRAM-unavailable event".into());
    }
    Ok(json!({"ready": ready, "handoff": handoff}))
}

fn profile_installed_neogeo_sdram(config: &NativeDeviceConfig, output_dir: &Path) -> Result<Value> {
    let session = connect_with(&config.connection, 10)?;
    fs::create_dir_all(output_dir)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: launch_return_once_initial_env(),
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    let status = read_launcher_status(&session)?;
    let main_status: Value = serde_json::from_str(
        &remote_read(&session, MAIN_STATUS_REMOTE).ok_or("Main status is missing")?,
    )?;
    let begun: Value = serde_json::from_str(&launcher_automation::begin(
        config,
        status
            .pointer("/build/version")
            .and_then(Value::as_str)
            .ok_or("NeoGeo smoke status has no build version")?,
        status
            .pointer("/build/source_revision")
            .and_then(Value::as_str)
            .ok_or("NeoGeo smoke status has no source revision")?,
        main_status
            .get("main_generation")
            .and_then(Value::as_u64)
            .ok_or("NeoGeo smoke Main status has no generation")?,
        120,
    )?)?;
    let mut nonce = begun
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or("NeoGeo smoke automation has no nonce")?
        .to_owned();

    let run_result = (|| -> Result<Value> {
        let initial = launch_return_once_open_neogeo(config, &nonce)?;
        let entries = collect_neogeo_entries(config, &nonce, initial)?;
        let targets = neogeo_smoke_matrix(&entries)?;
        fs::write(
            output_dir.join("discovered-neogeo-launches.json"),
            format!("{}\n", serde_json::to_string_pretty(&entries)?),
        )?;
        let mut runs = Vec::new();
        for (run_index, target) in targets.iter().enumerate() {
            let selected =
                launch_return_once_select_game_index(config, &nonce, target.selected_index)?;
            if modal_semantic(&selected, "selected_game_id").and_then(Value::as_str)
                != Some(target.game_id.as_str())
            {
                return Err(
                    format!("NeoGeo target changed before launch: {}", target.game_id).into(),
                );
            }
            let before_events = remote_read(&session, "/tmp/mister-magik/events.jsonl")
                .unwrap_or_default()
                .lines()
                .count();
            let capture_path = output_dir.join(format!(
                "{:02}-{}-active-usb-video.jpg",
                run_index + 1,
                target.role
            ));
            let returned = launcher_automation::exercise_launch_return_observed(
                config,
                &nonce,
                &target.game_id,
                120,
                NEOGEO_SDRAM_GAME_DWELL,
                || {
                    let capture = crate::capture::execute_analyzed(Some(&capture_path))?;
                    if capture.visibility != crate::capture::CaptureVisibility::Visible {
                        return Err(format!(
                            "active NeoGeo USB capture is not visible: {:?}",
                            capture.visibility
                        )
                        .into());
                    }
                    Ok(json!({
                        "usb_video": capture,
                        "operator": confirm_neogeo_game(target)?,
                    }))
                },
            )
            .map_err(|error| format!("NeoGeo {} run failed: {error}", target.role))?;
            let returned: Value = serde_json::from_str(&returned)?;
            nonce = returned
                .get("nonce")
                .and_then(Value::as_str)
                .ok_or("NeoGeo replacement session has no nonce")?
                .to_owned();
            let events = remote_read(&session, "/tmp/mister-magik/events.jsonl")
                .ok_or("Main events are missing after NeoGeo run")?;
            let sdram = validate_neogeo_sdram_events(&events, before_events)?;
            runs.push(json!({
                "role": target.role,
                "setname": target.setname,
                "game_id": target.game_id,
                "selected_index": target.selected_index,
                "sdram": sdram,
                "automation": returned,
            }));
        }
        let events = remote_read(&session, "/tmp/mister-magik/events.jsonl")
            .ok_or("Main events are missing after NeoGeo matrix")?;
        fs::write(output_dir.join("events.jsonl"), &events)?;
        Ok(json!({
            "schema": "mister-magik-neogeo-sdram-smoke-v1",
            "artifact_status": "passed",
            "required_sdram_size_code": 3,
            "runs": runs,
        }))
    })();
    let ended = launcher_automation::end(config, &nonce);
    match (run_result, ended) {
        (Ok(summary), Ok(_)) => {
            fs::write(
                output_dir.join("neogeo-sdram-smoke.json"),
                format!("{}\n", serde_json::to_string_pretty(&summary)?),
            )?;
            Ok(summary)
        }
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(end_error)) => {
            Err(format!("{error}; ending automation failed: {end_error}").into())
        }
    }
}

fn profile_installed_launch_return_once(
    config: &NativeDeviceConfig,
    output_dir: &Path,
) -> Result<String> {
    let session = connect_with(&config.connection, 10)?;
    fs::create_dir_all(output_dir)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: launch_return_once_initial_env(),
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            ..LauncherRestartOptions::default()
        },
    )?;
    let status = read_launcher_status(&session)?;
    let main_status: Value = serde_json::from_str(
        &remote_read(&session, MAIN_STATUS_REMOTE).ok_or("Main status is missing")?,
    )?;
    let build_version = status
        .pointer("/build/version")
        .and_then(Value::as_str)
        .ok_or("launch-return-once status has no build version")?;
    let source_revision = status
        .pointer("/build/source_revision")
        .and_then(Value::as_str)
        .ok_or("launch-return-once status has no source revision")?;
    let main_generation = main_status
        .get("main_generation")
        .and_then(Value::as_u64)
        .ok_or("launch-return-once Main status has no generation")?;
    let begun: Value = serde_json::from_str(&launcher_automation::begin(
        config,
        build_version,
        source_revision,
        main_generation,
        120,
    )?)?;
    let mut nonce = begun
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or("launch-return-once automation has no nonce")?
        .to_owned();

    let run_result = (|| -> Result<Value> {
        launch_return_once_action(config, &nonce, AutomationButton::Home)?;
        launch_return_once_select_menu_item(config, &nonce, "menu:arcade")?;
        launch_return_once_action(config, &nonce, AutomationButton::A)?;
        let selected = launch_return_once_select_game(config, &nonce)?;
        fs::write(
            output_dir.join("pre-launch-snapshot.json"),
            format!("{}\n", serde_json::to_string_pretty(&selected)?),
        )?;

        let returned = launcher_automation::exercise_launch_return(
            config,
            &nonce,
            LAUNCH_RETURN_ONCE_GAME,
            120,
            ATTENDED_LAUNCH_RETURN_GAME_DWELL,
        )
        .map_err(|error| format!("launch-return-once failed: {error}"))?;
        let returned: Value = serde_json::from_str(&returned)?;
        nonce = returned
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or("launch-return-once replacement session has no nonce")?
            .to_owned();
        let sequence = returned
            .get("post_return_action_sequence")
            .and_then(Value::as_u64)
            .ok_or("launch-return-once has no returned presentation sequence")?;

        // The return capsule can produce one correct frame before an
        // authoritative catalog publication reconciles the launcher. Require
        // the exact selection to remain intact beyond that publication edge.
        thread::sleep(Duration::from_millis(750));
        let restored_selection = launcher_automation::snapshot(config, &nonce)?;
        launch_return_once_validate_restored_selection(&selected, &restored_selection)?;
        fs::write(
            output_dir.join("restored-selection-snapshot.json"),
            format!("{}\n", serde_json::to_string_pretty(&restored_selection)?),
        )?;

        let returned_status = read_launcher_status(&session)?;
        fs::write(
            output_dir.join("returned-status.json"),
            format!("{}\n", serde_json::to_string_pretty(&returned_status)?),
        )?;
        let framebuffer: Value = serde_json::from_str(&launcher_automation::capture_checkpoint(
            config,
            &nonce,
            sequence,
            "returned-framebuffer",
            output_dir,
        )?)?;

        let usb_observation_started = Instant::now();
        let usb =
            crate::capture::execute_analyzed(Some(&output_dir.join("returned-usb-video.jpg")))?;
        let usb_json = serde_json::to_value(&usb)?;
        fs::write(
            output_dir.join("returned-usb-video.json"),
            format!("{}\n", serde_json::to_string_pretty(&usb_json)?),
        )?;
        let primary_usb_bytes = fs::read(&usb.artifact.path)?;
        let mut usb_confirmation = Vec::new();
        let mut confirmation_states = Vec::new();
        if matches!(
            usb.visibility,
            crate::capture::CaptureVisibility::Visible
                | crate::capture::CaptureVisibility::Corrupted
        ) {
            for index in 1..=LAUNCH_RETURN_PHYSICAL_CONFIRMATIONS {
                thread::sleep(LAUNCH_RETURN_PHYSICAL_CONFIRMATION_INTERVAL);
                let confirmation = crate::capture::execute_analyzed(Some(
                    &output_dir.join(format!("returned-usb-video-confirmation-{index}.jpg")),
                ))?;
                let identical_to_primary =
                    fs::read(&confirmation.artifact.path)? == primary_usb_bytes;
                let temporal_luma_delta_permille = usb.temporal_luma_delta_permille(&confirmation);
                confirmation_states.push(LaunchReturnUsbConfirmation {
                    visibility: confirmation.visibility,
                    temporal_luma_delta_permille,
                });
                usb_confirmation.push(json!({
                    "capture": confirmation,
                    "identical_to_primary": identical_to_primary,
                    "temporal_luma_delta_permille": temporal_luma_delta_permille,
                }));
            }
        }
        let effective_usb_visibility =
            launch_return_effective_usb_visibility(usb.visibility, &confirmation_states);
        let effective_usb_visibility_json = effective_usb_visibility
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or_else(|| json!("inconclusive"));
        let usb_observation_ms = usb_observation_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

        let diagnostics_reply = agent_request_at(
            config.agent()?,
            "diagnostics",
            json!({}),
            Duration::from_secs(3),
        )?;
        let diagnostics = diagnostics_reply
            .response
            .get("result")
            .cloned()
            .unwrap_or(Value::Null);
        fs::write(
            output_dir.join("fpga-video-diagnostics.json"),
            format!("{}\n", serde_json::to_string_pretty(&diagnostics)?),
        )?;
        for (remote, local) in [
            ("/tmp/mister-magik/events.jsonl", "events.jsonl"),
            ("/tmp/mister-magik-slint.log", "launcher.log"),
        ] {
            fs::write(
                output_dir.join(local),
                remote_read(&session, remote).ok_or_else(|| format!("missing {remote}"))?,
            )?;
        }

        let visible = effective_usb_visibility == Some(crate::capture::CaptureVisibility::Visible);
        Ok(json!({
            "schema": "mister-magik-launch-return-once-v2",
            "artifact_status": if visible { "passed" } else { "failed" },
            "game": LAUNCH_RETURN_ONCE_GAME,
            "cycles": 1,
            "returned": returned,
            "restored_selection": restored_selection,
            "returned_status": returned_status,
            "framebuffer": framebuffer,
            "fpga_video_diagnostics": diagnostics.get("fpga_video_diagnostics"),
            "usb_video": usb_json,
            "usb_video_effective_visibility": effective_usb_visibility_json,
            "usb_video_return_confirmation": {
                "schema": "mister-magik-return-physical-confirmation-v2",
                "required_confirmations": LAUNCH_RETURN_PHYSICAL_CONFIRMATIONS,
                "interval_ms": LAUNCH_RETURN_PHYSICAL_CONFIRMATION_INTERVAL.as_millis(),
                "observation_ms": usb_observation_ms,
                "temporal_luma_grid": crate::capture::TEMPORAL_LUMA_GRID_ID,
                "temporal_luma_corruption_threshold_permille": crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE,
                "captures": usb_confirmation,
            },
            "physical_video_visible": visible,
        }))
    })();

    let end_result = launcher_automation::end(config, &nonce);
    let summary = match (run_result, end_result) {
        (Ok(summary), Ok(_)) => summary,
        (Err(error), Ok(_)) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(format!("launch-return-once lease cleanup failed: {error}").into());
        }
        (Err(error), Err(cleanup)) => {
            return Err(
                format!("{error}; launch-return-once lease cleanup failed: {cleanup}").into(),
            );
        }
    };
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&summary)?),
    )?;
    serde_json::to_string(&summary).map_err(Into::into)
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

const RUNTIME_MANIFEST_FIELDS: &[&str] = platform_manifest_contract::FIELDS;

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
    let manifest = platform_manifest_contract::parse(
        &text,
        platform_manifest_contract::Layout::Development,
        platform_manifest_contract::ValidationProfile::AgentStrict,
    )
    .map_err(|error| format!("runtime manifest is invalid: {error}"))?;
    if manifest.get("gui_sha256") != Some(expected_sha256) {
        return Err("runtime manifest field gui_sha256 is not canonical".into());
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
    transferred_files: u64,
    transferred_bytes: u64,
    transfer_ms: u64,
    binary_transport: BinaryTransport,
    binary_transfer_ms: u64,
    agent_receive_ms: Option<u64>,
    agent_bytes_per_second: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BinaryTransport {
    AgentStream,
    AgentStreamReconciled,
    Sftp,
    SftpFallback,
}

impl BinaryTransport {
    fn label(self) -> &'static str {
        match self {
            Self::AgentStream => "agent-stream",
            Self::AgentStreamReconciled => "agent-stream-reconciled",
            Self::Sftp => "sftp",
            Self::SftpFallback => "sftp-fallback",
        }
    }
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
        if remote != DEVELOPMENT_GUI_REMOTE
            || manifest_remote != LOCAL_MAIN_MANIFEST_REMOTE
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
        agent: &AgentEndpoint,
        validate_ms: u128,
        total_t: Instant,
        metrics: &mut DeliveryTransferMetrics,
    ) -> Result<MagikDeployReport> {
        self.run_with(
            &SshDeployRemote {
                sess,
                agent: Some(agent),
            },
            validate_ms,
            total_t,
            metrics,
        )
    }

    fn run_with<R: DeployRemote>(
        &self,
        remote: &R,
        validate_ms: u128,
        total_t: Instant,
        metrics: &mut DeliveryTransferMetrics,
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
            suspend_runtime_launcher(remote)?;
            let suspend_ms = suspend_t.elapsed().as_millis();
            suspended = true;

            let upload_t = Instant::now();
            let transfer_before = metrics.upload_ms;
            let (binary_upload, binary_transfer_ms) = put_runtime_binary_measured(
                remote,
                &self.local,
                &self.upload,
                self.local_bytes,
                &self.expected_sha256,
                metrics,
            )?;
            let manifest_bytes = fs::metadata(&self.manifest.local)?.len();
            put_measured(
                remote,
                &self.manifest.local,
                &self.manifest.upload,
                manifest_bytes,
                metrics,
            )?;
            let transfer_ms = metrics.upload_ms.saturating_sub(transfer_before);
            self.verify_uploads(remote)?;
            let upload_ms = upload_t.elapsed().as_millis();

            let swap_ms = self.swap_upload(remote)?;
            let (chmod_size_ms, remote_bytes) = self.chmod_and_verify_size(remote)?;

            let cleanup_ms = self.cleanup(remote)?;
            cleaned = true;

            let resume_t = Instant::now();
            resume_runtime_launcher(remote)?;
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
                transferred_files: 2,
                transferred_bytes: self.local_bytes.saturating_add(manifest_bytes),
                transfer_ms,
                binary_transport: binary_upload.transport,
                binary_transfer_ms,
                agent_receive_ms: binary_upload.agent_receive_ms,
                agent_bytes_per_second: binary_upload.agent_bytes_per_second,
            })
        })();

        if result.is_err() {
            if !cleaned {
                let _ = self.cleanup(remote);
            }
            if suspended {
                let _ = resume_runtime_launcher(remote);
            }
        }
        result
    }

    fn prepare<R: DeployRemote>(&self, remote: &R) -> Result<u128> {
        let start = Instant::now();
        self.exec_phase(
            remote,
            "prepare",
            &format!(
                "mkdir -p {}; rm -f {} {}; : > {}",
                sh(&self.remote_dir),
                sh(&self.upload),
                sh(&format!("{}.part", self.upload)),
                sh(&self.lock)
            ),
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
                "rm -f {} {} {}{manifest_upload}",
                sh(&self.upload),
                sh(&format!("{}.part", self.upload)),
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

    fn put_runtime_binary(
        &self,
        local: &Path,
        remote: &str,
        _bytes: u64,
        _sha256: &str,
    ) -> Result<BinaryUploadResult> {
        self.put(local, remote)?;
        Ok(BinaryUploadResult::sftp(BinaryTransport::Sftp))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BinaryUploadResult {
    transport: BinaryTransport,
    agent_receive_ms: Option<u64>,
    agent_bytes_per_second: Option<u64>,
}

impl BinaryUploadResult {
    fn sftp(transport: BinaryTransport) -> Self {
        Self {
            transport,
            agent_receive_ms: None,
            agent_bytes_per_second: None,
        }
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn put_measured<R: DeployRemote>(
    remote: &R,
    local: &Path,
    destination: &str,
    bytes: u64,
    metrics: &mut DeliveryTransferMetrics,
) -> Result<()> {
    let started = Instant::now();
    let result = remote.put(local, destination);
    metrics.upload_ms = metrics.upload_ms.saturating_add(elapsed_millis(started));
    if result.is_ok() {
        metrics.files = metrics.files.saturating_add(1);
        metrics.bytes = metrics.bytes.saturating_add(bytes);
    }
    result
}

fn put_runtime_binary_measured<R: DeployRemote>(
    remote: &R,
    local: &Path,
    destination: &str,
    bytes: u64,
    sha256: &str,
    metrics: &mut DeliveryTransferMetrics,
) -> Result<(BinaryUploadResult, u64)> {
    let started = Instant::now();
    let result = remote.put_runtime_binary(local, destination, bytes, sha256);
    let elapsed = elapsed_millis(started);
    metrics.upload_ms = metrics.upload_ms.saturating_add(elapsed);
    if result.is_ok() {
        metrics.files = metrics.files.saturating_add(1);
        metrics.bytes = metrics.bytes.saturating_add(bytes);
    }
    result.map(|transport| (transport, elapsed))
}

struct SshDeployRemote<'a> {
    sess: &'a Session,
    agent: Option<&'a AgentEndpoint>,
}

impl DeployRemote for SshDeployRemote<'_> {
    fn exec(&self, command: &str) -> Result<ExecOutput> {
        exec(self.sess, command, true)
    }

    fn put(&self, local: &Path, remote: &str) -> Result<()> {
        put(self.sess, local, remote)
    }

    fn put_runtime_binary(
        &self,
        local: &Path,
        remote: &str,
        bytes: u64,
        sha256: &str,
    ) -> Result<BinaryUploadResult> {
        let Some(agent) = self.agent else {
            self.put(local, remote)?;
            return Ok(BinaryUploadResult::sftp(BinaryTransport::Sftp));
        };
        match agent_runtime_upload_at(agent, local, bytes, sha256, Duration::from_secs(120)) {
            Ok(result) => Ok(BinaryUploadResult {
                transport: BinaryTransport::AgentStream,
                agent_receive_ms: Some(result.receive_ms),
                agent_bytes_per_second: Some(result.bytes_per_second),
            }),
            Err(upload_error) => {
                let reconcile = self.exec(&format!(
                    "if test -f {0} && test \"$(wc -c < {0})\" = {1} && test \"$(sha256sum {0} | awk '{{print $1}}')\" = {2}; then echo exact; else echo mismatch; fi",
                    sh(remote),
                    bytes,
                    sh(sha256),
                ))?;
                if reconcile.rc == 0 && reconcile.stdout.trim() == "exact" {
                    return Ok(BinaryUploadResult::sftp(
                        BinaryTransport::AgentStreamReconciled,
                    ));
                }
                let part = format!("{remote}.part");
                let cleanup = self.exec(&format!(
                    "rm -f {0} {1}; test ! -e {0}; test ! -e {1}",
                    sh(remote),
                    sh(&part),
                ))?;
                if cleanup.rc != 0 {
                    return Err(format!(
                        "runtime agent upload failed ({upload_error}); reconciliation cleanup failed rc={} output={}",
                        cleanup.rc,
                        cleanup.stdout.trim()
                    )
                    .into());
                }
                eprintln!(
                    "runtime agent upload failed; reconciled staging and using one SFTP fallback: {upload_error}"
                );
                self.put(local, remote)?;
                Ok(BinaryUploadResult::sftp(BinaryTransport::SftpFallback))
            }
        }
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

fn suspend_runtime_launcher<R: DeployRemote>(remote: &R) -> Result<()> {
    deploy_fifo_command(remote, "mister_magik_suspend")
}

fn resume_runtime_launcher<R: DeployRemote>(remote: &R) -> Result<()> {
    deploy_fifo_command(remote, "mister_magik_resume")
}

impl MagikDeployReport {
    fn print(&self) {
        let finish_ms = self.swap_ms + self.chmod_size_ms;
        let resume_size_ms = self.resume_ms + self.chmod_size_ms;
        println!(
            "deploy_runtime_bundle local={} remote={} local_bytes={} remote_bytes={} total_ms={} prepare_ms={} suspend_ms={} put_ms={} finish_ms={} resume_size_ms={} validate_ms={} upload_ms={} swap_ms={} chmod_size_ms={} resume_ms={} cleanup_ms={} transferred_files={} transferred_bytes={} transfer_ms={} binary_transport={} binary_transfer_ms={} agent_receive_ms={} agent_bytes_per_second={}",
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
            self.cleanup_ms,
            self.transferred_files,
            self.transferred_bytes,
            self.transfer_ms,
            self.binary_transport.label(),
            self.binary_transfer_ms,
            self.agent_receive_ms
                .map_or_else(|| "n/a".to_string(), |value| value.to_string()),
            self.agent_bytes_per_second
                .map_or_else(|| "n/a".to_string(), |value| value.to_string()),
        );
    }
}

fn parse_wc_byte_count(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse::<u64>().ok()
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
    elapsed_ms: u128,
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

fn capture_output_has_existing_artifact(path: &Path) -> Result<bool> {
    let stem = normalize_capture_stem(path)?;
    Ok(["-raw.png", "-raw-letterbox-4x3.png", "-display-4x3.png"]
        .iter()
        .any(|suffix| capture_artifact_path(&stem, suffix).exists()))
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

fn write_capture_manifest(path: &Path, manifest: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
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
    if args.is_empty() || (args.len() == 2 && args[0] == "--output" && !args[1].trim().is_empty()) {
        Ok(())
    } else {
        Err("usage: scripts/agent device capture framebuffer [--output STEM]".into())
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

fn capture_and_restore_launcher(
    config: &NativeDeviceConfig,
    output: &Path,
    label: &str,
) -> Result<()> {
    let capture = capture_buffer_at(
        config.agent()?,
        &["--output".into(), output.to_string_lossy().into_owned()],
    );
    let cleanup = restore_launcher_after_fixture(config);
    match (capture, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(capture), Ok(())) => Err(capture),
        (Ok(()), Err(cleanup)) => {
            Err(format!("{label} was captured but launcher cleanup failed: {cleanup}").into())
        }
        (Err(capture), Err(cleanup)) => Err(format!(
            "{label} capture failed ({capture}); launcher cleanup also failed ({cleanup})"
        )
        .into()),
    }
}

fn restore_launcher_after_fixture(config: &NativeDeviceConfig) -> Result<()> {
    connect_with(&config.connection, 10).and_then(|session| {
        launcher_restart(
            &session,
            &LauncherRestartOptions {
                clear_env: true,
                ..LauncherRestartOptions::default()
            },
        )
    })
}

fn capture_first_arcade(config: &NativeDeviceConfig, output: &Path) -> Result<()> {
    capture_arcade_variant(config, output, None, "home", None, "first Arcade screen")
}

fn first_arcade_capture_ready(status: &Value, experiment: Option<&str>) -> bool {
    status.get("screen").and_then(Value::as_str) == Some("arcade")
        && status.get("composition_state").and_then(Value::as_str) == Some("mixed-arcade")
        && status.get("crt_font_experiment").and_then(Value::as_str)
            == Some(experiment.unwrap_or("baseline"))
        && status
            .get("selected_game_id")
            .and_then(Value::as_str)
            .is_some_and(|game| !game.is_empty())
        && status
            .get("selected_game_has_preview")
            .and_then(Value::as_bool)
            == Some(true)
        && status.get("preview_cache_state").and_then(Value::as_str) == Some("exact")
        && status
            .get("preview_presentation_state")
            .and_then(Value::as_str)
            == Some("visible")
}

fn capture_crt_font_ab(config: &NativeDeviceConfig, pair: &str, output: &Path) -> Result<()> {
    if pair != "row-phase" {
        return Err("unsupported CRT font pair; expected row-phase".into());
    }
    let base = normalize_capture_stem(output)?;
    let parent = base
        .parent()
        .ok_or("CRT font A/B output must have a parent directory")?;
    if !parent.is_dir() {
        return Err(format!(
            "CRT font A/B output directory does not exist: {}",
            parent.display()
        )
        .into());
    }
    let base_name = base
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("CRT font A/B output stem must be valid UTF-8")?;
    let a_stem = base.with_file_name(format!("{base_name}-row-phase-a-odd"));
    let b_stem = base.with_file_name(format!("{base_name}-row-phase-b-even"));
    let compare_path = base.with_file_name(format!("{base_name}-row-phase-compare-4x3.png"));
    let manifest_path = base.with_file_name(format!("{base_name}-row-phase.json"));
    for stem in [&a_stem, &b_stem] {
        if !capture_paths_available(stem, true) {
            return Err(format!(
                "CRT font A/B capture output already exists: {}",
                stem.display()
            )
            .into());
        }
    }
    if compare_path.exists() || manifest_path.exists() {
        return Err("CRT font A/B comparison output already exists".into());
    }

    let original_mode = active_display_mode_id()?;
    let original_mode_known = DISPLAY_MATRIX_MODES
        .iter()
        .any(|mode| mode.id == original_mode);
    if !original_mode_known {
        return Err(format!("cannot restore unsupported display mode: {original_mode}").into());
    }
    let switched_to_240 = original_mode != "crt-240p60";
    if switched_to_240 {
        display_mode_cli(&["crt-240p60".into(), "--attended".into(), "--keep".into()])?;
    }

    let capture_result = (|| -> Result<()> {
        capture_arcade_variant(
            config,
            &a_stem,
            None,
            "arcade",
            Some("baseline"),
            "CRT font row phase A (odd rows)",
        )?;
        capture_arcade_variant(
            config,
            &b_stem,
            None,
            "arcade",
            Some("phase-even"),
            "CRT font row phase B (even rows)",
        )?;
        let a_display = fs::read(capture_artifact_path(&a_stem, "-display-4x3.png"))?;
        let b_display = fs::read(capture_artifact_path(&b_stem, "-display-4x3.png"))?;
        let comparison = framebuffer_views::side_by_side_4x3_png(&a_display, &b_display)?;
        write_capture_files(&[PendingCaptureArtifact {
            label: "CRT font row phase A/B comparison 4:3",
            path: compare_path.clone(),
            png: comparison,
        }])?;
        let manifest = json!({
            "schema": "mister-magik-crt-font-ab-v1",
            "pair": pair,
            "route": "crt-240p60",
            "a": {"label": "odd rows", "experiment": "baseline", "stem": a_stem},
            "b": {"label": "even rows", "experiment": "phase-even", "stem": b_stem},
            "comparison": compare_path,
        });
        write_capture_manifest(&manifest_path, &manifest)
    })();
    let restore_result = if switched_to_240 {
        display_mode_cli(&[original_mode, "--attended".into(), "--keep".into()])
    } else {
        Ok(())
    };
    match (capture_result, restore_result) {
        (Ok(()), Ok(())) => {
            println!("CRT font A/B comparison: {}", compare_path.display());
            println!("CRT font A/B manifest: {}", manifest_path.display());
            Ok(())
        }
        (Err(capture), Ok(())) => Err(capture),
        (Ok(()), Err(restore)) => Err(format!(
            "CRT font A/B capture succeeded but display restore failed: {restore}"
        )
        .into()),
        (Err(capture), Err(restore)) => Err(format!(
            "CRT font A/B capture failed ({capture}); display restore also failed ({restore})"
        )
        .into()),
    }
}

fn active_display_mode_id() -> Result<String> {
    let session = connect(10)?;
    let reply = exec_checked_output(
        &session,
        "query original display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    Ok(parse_display_reply_active(reply.stdout.trim())?.to_string())
}

fn capture_arcade_variant(
    config: &NativeDeviceConfig,
    output: &Path,
    catalog_refresh: Option<&str>,
    start_screen: &str,
    experiment: Option<&str>,
    label: &str,
) -> Result<()> {
    if capture_output_has_existing_artifact(output)? {
        return Err(format!("capture output already exists: {}", output.display()).into());
    }
    let session = connect_with(&config.connection, 10)?;
    let mut env_vars = vec![("MISTER_LAUNCHER_START_SCREEN".into(), start_screen.into())];
    if let Some(catalog_refresh) = catalog_refresh {
        env_vars.insert(0, ("MISTER_CATALOG_REFRESH".into(), catalog_refresh.into()));
    }
    if let Some(experiment) = experiment {
        env_vars.push(("MISTER_CRT_FONT_EXPERIMENT".into(), experiment.into()));
    }
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars,
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            ..LauncherRestartOptions::default()
        },
    )?;

    let fixture = (|| -> Result<()> {
        let status = read_launcher_status(&session)?;
        let main_status: Value = serde_json::from_str(
            &remote_read(&session, MAIN_STATUS_REMOTE).ok_or("Main status is missing")?,
        )?;
        let begin = launcher_automation::begin(
            config,
            status
                .pointer("/build/version")
                .and_then(Value::as_str)
                .ok_or("Arcade capture status has no build version")?,
            status
                .pointer("/build/source_revision")
                .and_then(Value::as_str)
                .ok_or("Arcade capture status has no source revision")?,
            main_status
                .get("main_generation")
                .and_then(Value::as_u64)
                .ok_or("Arcade capture Main status has no generation")?,
            30,
        )?;
        let begin: Value = serde_json::from_str(&begin)?;
        let nonce = begin
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or("Arcade capture automation has no nonce")?
            .to_owned();
        let navigate = (|| -> Result<()> {
            if status.get("screen").and_then(Value::as_str) != Some("arcade") {
                modal_input_action(config, &nonce, AutomationAction::Tap(AutomationButton::A))?;
            }
            Ok(())
        })();
        let end = launcher_automation::end(config, &nonce).map(|_| ());
        match (navigate, end) {
            (Ok(()), Ok(())) => {}
            (Err(navigate), Ok(())) => return Err(navigate),
            (Ok(()), Err(end)) => {
                return Err(format!(
                    "Arcade capture navigation completed but automation cleanup failed: {end}"
                )
                .into());
            }
            (Err(navigate), Err(end)) => {
                return Err(format!(
                    "Arcade capture navigation failed ({navigate}); automation cleanup also failed ({end})"
                )
                .into());
            }
        }

        (|| -> Result<()> {
            let started = Instant::now();
            let timeout = Duration::from_secs(15);
            loop {
                let status = read_launcher_status(&session)?;
                let arcade_settled = first_arcade_capture_ready(&status, experiment);
                if arcade_settled {
                    return Ok(());
                }
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "Arcade capture did not settle within {} ms; final status={status}",
                        started.elapsed().as_millis()
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(20));
            }
        })()
    })();
    drop(session);

    if let Err(fixture) = fixture {
        return match restore_launcher_after_fixture(config) {
            Ok(()) => Err(fixture),
            Err(cleanup) => Err(format!(
                "{label} fixture failed ({fixture}); launcher cleanup also failed ({cleanup})"
            )
            .into()),
        };
    }

    capture_and_restore_launcher(config, output, label)
}

fn capture_snes_hub(config: &NativeDeviceConfig, output: &Path) -> Result<()> {
    if capture_output_has_existing_artifact(output)? {
        return Err(format!("capture output already exists: {}", output.display()).into());
    }
    let session = connect_with(&config.connection, 10)?;
    restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                ("MISTER_LAUNCHER_START_SCREEN".into(), "system-hub".into()),
                ("MISTER_LAUNCHER_START_SYSTEM".into(), "snes".into()),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV_REMOTE.as_str().into(),
            ..LauncherRestartOptions::default()
        },
    )?;

    let started = Instant::now();
    let timeout = Duration::from_secs(15);
    loop {
        let status = read_launcher_status(&session)?;
        if status.get("screen").and_then(Value::as_str) == Some("system-hub") {
            break;
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "SNES hub did not settle within {} ms; final status={status}",
                started.elapsed().as_millis()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    drop(session);

    capture_and_restore_launcher(config, output, "SNES hub")
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

fn wait_main_launcher_active(sess: &Session, timeout: Duration) -> Result<u128> {
    let started = Instant::now();
    let mut last_state = String::new();
    while started.elapsed() < timeout {
        let main = remote_read(sess, MAIN_STATUS_REMOTE)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        last_state = main
            .as_ref()
            .and_then(|status| status.get("launcher_state"))
            .and_then(Value::as_str)
            .unwrap_or("missing")
            .to_string();
        if last_state == "LauncherActive" {
            return Ok(started.elapsed().as_millis());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "Main did not return to LauncherActive after {}ms; last launcher_state={last_state}",
        timeout.as_millis()
    )
    .into())
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

fn active_installed_gui_binary(status: &Value) -> Result<&'static str> {
    if status.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive") {
        return Err("active runtime selection requires an active launcher".into());
    }
    if status.get("fpga_owner").and_then(Value::as_str) != Some("magik") {
        return Err("active runtime selection requires MagiK to own the FPGA".into());
    }
    match status.get("executable_path").and_then(Value::as_str) {
        Some(LOCAL_MAIN_REMOTE) => Ok(DEVELOPMENT_GUI_REMOTE),
        Some(PUBLIC_MAIN_REMOTE) => Ok(PUBLIC_GUI_REMOTE),
        Some(path) => Err(format!("unsupported active Main executable: {path}").into()),
        None => Err("active Main status does not identify its executable".into()),
    }
}

fn display_route_status(sess: &Session) -> Result<()> {
    let status_text = remote_read(sess, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for display route readback")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    for (label, subcommand) in [
        ("display route readback", "read"),
        ("latched framebuffer readback", "fpga-latch-report"),
    ] {
        let command = remote_subcommand(binary, subcommand, &[]);
        let out = exec(sess, &command, true)?;
        print!("{}", out.stdout);
        if !out.stderr.trim().is_empty() {
            eprint!("[stderr] {}", out.stderr);
        }
        if let Some(error) = exec_failure_message(label, &out) {
            return Err(error.into());
        }
    }
    Ok(())
}

fn parse_last_json_line(label: &str, output: &str) -> Result<Value> {
    output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())
        .ok_or_else(|| format!("{label} produced no JSON record").into())
}

fn run_catalog_inspect(sess: &Session, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err("usage: scripts/agent device catalog inspect".into());
    }
    let status_text = remote_read(sess, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for catalog inspection")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    let command = remote_subcommand(binary, "catalog-inspect", args);
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

fn run_runtime_metadata_qualification(sess: &Session, output: &Path) -> Result<()> {
    let status_text = remote_read(sess, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for metadata qualification")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    let command = remote_subcommand(binary, "metadata-qualification-report", &[]);
    let out = exec(sess, &command, true)?;
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    if let Some(error) = exec_failure_message("runtime metadata qualification", &out) {
        return Err(error.into());
    }
    let report = parse_last_json_line("runtime metadata qualification", &out.stdout)?;
    let mut evidence = report;
    evidence["legacy_sqlite_absence"] = runtime_metadata_legacy_sqlite_evidence(sess)?;
    let validation = validate_runtime_metadata_qualification(&evidence);
    if validation.is_ok() {
        evidence["device_acceptance"] = json!({
            "complete": true,
            "mode": "compact-only",
            "compact_integrity": true,
            "legacy_sqlite_absence": true,
        });
    }
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output,
        format!("{}\n", serde_json::to_string_pretty(&evidence)?),
    )?;
    validation?;
    println!(
        "runtime_metadata_qualification=passed compact_only=true complete=true legacy_sqlite_absent=true bytes={} shards={} evidence={}",
        evidence
            .pointer("/compact/file_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        evidence
            .pointer("/compact/shard_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output.display(),
    );
    Ok(())
}

fn runtime_metadata_legacy_sqlite_evidence(sess: &Session) -> Result<Value> {
    let sftp = sess.sftp()?;
    let mut paths = Vec::with_capacity(LEGACY_RUNTIME_METADATA_PATHS.len());
    for path in LEGACY_RUNTIME_METADATA_PATHS {
        let present = match sftp.stat(Path::new(path)) {
            Ok(_) => true,
            Err(error) => {
                let io_error: io::Error = error.into();
                if io_error.kind() == io::ErrorKind::NotFound {
                    false
                } else {
                    return Err(format!("stat {path}: {io_error}").into());
                }
            }
        };
        paths.push(json!({"path": path, "present": present}));
    }
    let all_absent = paths.iter().all(|path| path["present"] == false);
    Ok(json!({
        "schema": "mister-magik-runtime-metadata-legacy-sqlite-absence-v1",
        "paths": paths,
        "all_absent": all_absent,
    }))
}

fn validate_runtime_metadata_qualification(report: &Value) -> Result<()> {
    if report.get("schema").and_then(Value::as_str)
        != Some("mister-magik-runtime-metadata-qualification-v2")
        || report.pointer("/compact/valid").and_then(Value::as_bool) != Some(true)
        || report
            .pointer("/compact/shard_count")
            .and_then(Value::as_u64)
            != Some(35)
        || report
            .pointer("/compact/file_bytes")
            .and_then(Value::as_u64)
            .is_none_or(|bytes| bytes == 0 || bytes > 8 * 1024 * 1024)
        || report
            .pointer("/compact/software_rows")
            .and_then(Value::as_u64)
            .is_none_or(|rows| rows == 0)
        || report
            .pointer("/compact/arcade_mame_rows")
            .and_then(Value::as_u64)
            .is_none_or(|rows| rows == 0)
        || report
            .pointer("/compact/arcade_hbmame_rows")
            .and_then(Value::as_u64)
            .is_none_or(|rows| rows == 0)
        || report
            .pointer("/compact/arcade_mister_rows")
            .and_then(Value::as_u64)
            .is_none_or(|rows| rows == 0)
    {
        return Err("runtime metadata qualification report failed compact integrity gates".into());
    }
    validate_runtime_metadata_legacy_sqlite_absence(
        report
            .get("legacy_sqlite_absence")
            .ok_or("runtime metadata qualification report is missing legacy SQLite evidence")?,
    )?;
    Ok(())
}

fn validate_runtime_metadata_legacy_sqlite_absence(evidence: &Value) -> Result<()> {
    if evidence.get("schema").and_then(Value::as_str)
        != Some("mister-magik-runtime-metadata-legacy-sqlite-absence-v1")
        || evidence.get("all_absent").and_then(Value::as_bool) != Some(true)
    {
        return Err(
            "runtime metadata qualification report failed legacy SQLite absence gate".into(),
        );
    }
    let paths = evidence
        .get("paths")
        .and_then(Value::as_array)
        .ok_or("runtime metadata qualification report is missing legacy SQLite paths")?;
    if paths.len() != LEGACY_RUNTIME_METADATA_PATHS.len() {
        return Err(
            "runtime metadata qualification report has the wrong legacy SQLite path count".into(),
        );
    }
    let mut seen = BTreeSet::new();
    for entry in paths {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or("runtime metadata qualification legacy SQLite path is missing")?;
        if !LEGACY_RUNTIME_METADATA_PATHS.contains(&path) || !seen.insert(path) {
            return Err(
                "runtime metadata qualification report has an unexpected legacy SQLite path".into(),
            );
        }
        if entry.get("present").and_then(Value::as_bool) != Some(false) {
            return Err(format!("forbidden legacy SQLite metadata path is present: {path}").into());
        }
    }
    if seen.len() != LEGACY_RUNTIME_METADATA_PATHS.len() {
        return Err("runtime metadata qualification report is missing a legacy SQLite path".into());
    }
    Ok(())
}

fn run_catalog_rom_audit(sess: &Session, output: &Path) -> Result<()> {
    let status_text = remote_read(sess, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for Arcade ROM audit")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    let command = remote_subcommand(binary, "catalog-arcade-rom-audit", &[]);
    let out = exec(sess, &command, true)?;
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    if let Some(error) = exec_failure_message("Arcade ROM visibility audit", &out) {
        return Err(error.into());
    }
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &out.stdout)?;
    println!("arcade_rom_visibility_report={}", output.display());
    if let Some(summary) = out
        .stdout
        .lines()
        .find(|line| line.starts_with("arcade_rom_visibility_summary_tsv\t"))
    {
        println!("{summary}");
    }
    Ok(())
}

fn run_catalog_neogeo_family_audit(sess: &Session, output: &Path) -> Result<()> {
    let status_text = remote_read(sess, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for Neo Geo family audit")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    let command = remote_subcommand(binary, "catalog-neogeo-family-audit", &[]);
    let out = exec(sess, &command, true)?;
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &out.stdout)?;
    println!("neogeo_family_report={}", output.display());
    if let Some(summary) = out
        .stdout
        .lines()
        .find(|line| line.starts_with("neogeo_family_summary_tsv\t"))
    {
        println!("{summary}");
    }
    if let Some(error) = exec_failure_message("Neo Geo family audit", &out) {
        return Err(error.into());
    }
    Ok(())
}

fn run_catalog_screenshot_export(system: &str, output: &Path) -> Result<()> {
    let system_id = mister_magik_catalog::catalog_classify::SystemId::parse(system)?;
    let session = connect(10)?;
    let status_text = remote_read(&session, MAIN_STATUS_REMOTE)
        .ok_or("active Main status is unavailable for screenshot audit")?;
    let status: Value = serde_json::from_str(&status_text)?;
    let binary = active_installed_gui_binary(&status)?;
    let command = remote_subcommand(
        binary,
        "catalog-screenshot-audit",
        &[system_id.as_str().to_string()],
    );
    let out = exec(&session, &command, true)?;
    if !out.stderr.trim().is_empty() {
        eprint!("[stderr] {}", out.stderr);
    }
    if let Some(error) = exec_failure_message("catalog screenshot audit", &out) {
        return Err(error.into());
    }
    let report = catalog_screenshot_tsv(&out.stdout)?;
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, report)?;
    println!(
        "catalog_screenshot_export_tsv\tsystem={}\tout={}",
        system_id.as_str(),
        output.display(),
    );
    Ok(())
}

fn catalog_screenshot_tsv(stdout: &str) -> Result<String> {
    const HEADER: &str =
        "ordinal\ttitle\tpreview_asset_key\tpreview_archive_path\thas_preview\tlaunch_ref";
    let mut rows = Vec::new();
    for line in stdout.lines() {
        if line == HEADER
            || line
                .split('\t')
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .is_some()
        {
            rows.push(line);
        }
    }
    if rows.first().copied() != Some(HEADER) {
        return Err("device returned no screenshot audit TSV".into());
    }
    Ok(format!("{}\n", rows.join("\n")))
}

fn purge_development_library_data_and_reboot() -> Result<()> {
    let session = connect(10)?;
    purge_development_library_data(&session)?;
    drop(session);
    agent_reboot_wait(&[])
}

fn purge_development_library_data(session: &Session) -> Result<()> {
    let preflight = format!(
        "set -eu; pidof MiSTer_MagiKDev >/dev/null; test -x {gui}; test ! -e /tmp/mister-magik/reboot-unstable; {safety}",
        gui = sh(DEVELOPMENT_GUI_REMOTE),
        safety = platform_safety_script(),
    );
    exec_checked(session, "Dev library purge preflight", &preflight)?;
    exec_checked(
        session,
        "Dev library purge suspend",
        &acknowledged_main_command("mister_magik_suspend"),
    )?;
    let purge = exec_checked_output(
        session,
        "Dev catalog and screenshot purge",
        &format!(
            "{} purge-library-data --confirm",
            sh(DEVELOPMENT_GUI_REMOTE)
        ),
    );
    let purge = match purge {
        Ok(output) => output,
        Err(error) => {
            let resume = exec_checked(
                session,
                "Dev library purge recovery resume",
                &acknowledged_main_command("mister_magik_resume"),
            );
            return match resume {
                Ok(()) => Err(error),
                Err(resume) => Err(format!(
                    "Dev library purge failed ({error}); launcher recovery also failed ({resume})"
                )
                .into()),
            };
        }
    };
    let summary = purge.stdout.trim();
    if !summary.lines().any(|line| {
        line.starts_with("purge_library_data\tdone\tcatalog_removed=")
            && line.contains("\tscreenshot_removed=")
    }) {
        let resume = exec_checked(
            session,
            "Dev library purge invalid-output recovery resume",
            &acknowledged_main_command("mister_magik_resume"),
        );
        let error = "Dev library purge did not report its guarded completion marker";
        return match resume {
            Ok(()) => Err(error.into()),
            Err(resume) => Err(format!("{error}; launcher recovery also failed ({resume})").into()),
        };
    }
    println!("{summary}");
    exec_checked(session, "sync Dev library purge", "sync")?;
    exec_checked(
        session,
        "Dev library purge resume before reboot",
        &acknowledged_main_command("mister_magik_resume"),
    )?;
    let active_ms = wait_main_launcher_active(session, Duration::from_secs(20))?;
    println!("Dev library purge Main active after {active_ms}ms");
    Ok(())
}

fn catalog_query(args: &[String]) -> Result<()> {
    if args.len() != 4 {
        return Err(
            "catalog query requires --database <registry|library|system:ID> --sql SQL".into(),
        );
    }
    let database = option_value(args, "--database")
        .ok_or("catalog query requires --database <registry|library|system:ID>")?;
    let sql = option_value(args, "--sql").ok_or("catalog query requires --sql SQL")?;
    let session = connect(10)?;
    let remote_root = active_catalog_root(&session)?;
    let temporary = CatalogQueryTemporary::create()?;
    let remote_database =
        resolve_catalog_database(&session, &remote_root, &database, temporary.path())?;
    let local_database = temporary.path().join("snapshot.sqlite3");
    snapshot_catalog_database(&session, &remote_database, &local_database)?;
    let connection = Connection::open_with_flags(
        &local_database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let output = mister_magik_catalog::sqlite_inspect::sqlite_query_to_tsv(&connection, &sql)
        .map_err(|error| {
            format!("catalog query must be one read-only SELECT or PRAGMA: {error}")
        })?;
    print!("{output}");
    Ok(())
}

fn snapshot_catalog_database(
    session: &Session,
    remote_database: &str,
    local_database: &Path,
) -> Result<()> {
    if !is_catalog_database_path(remote_database) {
        return Err("catalog snapshot source is outside the active catalog".into());
    }
    let live_database = local_database.with_file_name("live.sqlite3");
    let remote_files = [
        ("db", remote_database.to_owned(), live_database.clone()),
        (
            "wal",
            format!("{remote_database}-wal"),
            live_database.with_file_name("live.sqlite3-wal"),
        ),
        (
            "shm",
            format!("{remote_database}-shm"),
            live_database.with_file_name("live.sqlite3-shm"),
        ),
    ];
    let mut stable = false;
    for _ in 0..3 {
        let before = catalog_snapshot_identity(session, &remote_files)?;
        for (role, remote, local) in &remote_files {
            if before.get(*role).is_some_and(|hash| hash != "missing") {
                get(session, remote, local)?;
            } else {
                let _ = fs::remove_file(local);
            }
        }
        if before == catalog_snapshot_identity(session, &remote_files)? {
            stable = true;
            break;
        }
    }
    if !stable {
        return Err("catalog database changed during the bounded snapshot transfer".into());
    }
    let source = Connection::open_with_flags(
        &live_database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut destination = Connection::open(local_database)?;
    Backup::new(&source, &mut destination)?.run_to_completion(
        64,
        Duration::from_millis(10),
        None,
    )?;
    Ok(())
}

fn catalog_snapshot_identity(
    session: &Session,
    files: &[(&str, String, PathBuf); 3],
) -> Result<BTreeMap<String, String>> {
    let command = files
        .iter()
        .map(|(role, path, _)| {
            format!(
                "if test -f {path}; then printf '{role}\\t'; sha256sum {path} | cut -d' ' -f1; else printf '{role}\\tmissing\\n'; fi",
                path = sh(path),
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let output = exec(session, &command, false)?;
    if let Some(message) = exec_failure_message("catalog snapshot identity", &output) {
        return Err(message.into());
    }
    let mut identity = BTreeMap::new();
    for line in output.stdout.lines() {
        let (role, hash) = line
            .split_once('\t')
            .ok_or("catalog snapshot identity returned an invalid line")?;
        if !matches!(role, "db" | "wal" | "shm")
            || (hash != "missing"
                && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())))
        {
            return Err("catalog snapshot identity returned invalid data".into());
        }
        identity.insert(role.to_owned(), hash.to_owned());
    }
    if identity.len() != files.len() || identity.get("db").is_none_or(|hash| hash == "missing") {
        return Err("catalog snapshot source is missing or incomplete".into());
    }
    Ok(identity)
}

fn is_catalog_database_path(path: &str) -> bool {
    let relative = [Layout::Public, Layout::Development]
        .into_iter()
        .map(|layout| {
            format!(
                "{}/",
                installed_layout::app_path(layout, "catalog-fast-v1")
                    .expect("static installed path")
            )
        })
        .find_map(|root| path.strip_prefix(&root).map(str::to_owned));
    relative.as_deref().is_some_and(|path| {
        !path.is_empty()
            && !path.starts_with('/')
            && !path.split('/').any(|component| component == "..")
            && path.ends_with(".sqlite3")
    })
}

fn active_catalog_root(session: &Session) -> Result<String> {
    let public = installed_layout::app_path(Layout::Public, "catalog-fast-v1")?;
    let development = installed_layout::app_path(Layout::Development, "catalog-fast-v1")?;
    let output = exec(
        session,
        &format!(
            "set -eu; if pidof MiSTer_MagiKDev >/dev/null 2>&1; then printf %s {development}; else printf %s {public}; fi",
            development = sh(&development),
            public = sh(&public),
        ),
        false,
    )?;
    if let Some(message) = exec_failure_message("resolve catalog root", &output) {
        return Err(message.into());
    }
    let root = output.stdout.trim();
    if root != public && root != development {
        return Err("device returned an invalid catalog root".into());
    }
    Ok(root.to_owned())
}

fn resolve_catalog_database(
    session: &Session,
    remote_root: &str,
    database: &str,
    temporary: &Path,
) -> Result<String> {
    match database {
        "registry" => Ok(format!("{remote_root}/state/catalog-state.sqlite3")),
        "library" => Ok(format!("{remote_root}/state/scanner-cache.sqlite3")),
        value if value.starts_with("system:") => {
            let system_id = value.trim_start_matches("system:");
            if system_id.is_empty()
                || !system_id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(
                    "catalog system id must contain only lowercase letters, digits, or '-'".into(),
                );
            }
            let registry = temporary.join("registry");
            fs::create_dir_all(&registry)?;
            let mut slots = 0_u8;
            for slot in ["manifest-a.json", "manifest-b.json"] {
                if get(
                    session,
                    &format!("{remote_root}/registry/{slot}"),
                    &registry.join(slot),
                )
                .is_ok()
                {
                    slots += 1;
                }
            }
            if slots == 0 {
                return Err("device catalog has no readable registry manifest".into());
            }
            let manifest = mister_magik_catalog::shard_registry::read_latest_manifest_lazy(
                temporary,
                mister_magik_catalog::shard_registry::production_registry_limits(),
            )?;
            let relative = manifest
                .systems
                .into_iter()
                .find(|system| system.system_id.as_str() == system_id)
                .and_then(|system| system.active.sqlite_path)
                .ok_or_else(|| format!("catalog has no active system '{system_id}'"))?;
            let relative = relative
                .to_str()
                .filter(|path| !path.starts_with('/') && !path.contains(".."))
                .ok_or("catalog manifest contains an invalid system database path")?;
            Ok(format!("{remote_root}/{relative}"))
        }
        _ => Err("catalog database must be registry, library, or system:ID".into()),
    }
}

struct CatalogQueryTemporary(PathBuf);

impl CatalogQueryTemporary {
    fn create() -> Result<Self> {
        let path = env::temp_dir().join(format!(
            "mister-magik-catalog-query-{}-{}",
            std::process::id(),
            unix_ms_now()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CatalogQueryTemporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
    SelectMain(String),
    MenuMode(String),
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

fn ensure_stock_inittab(sess: &Session, dry_run: bool) -> Result<()> {
    const INITTAB: &str = "/etc/inittab";
    let input = remote_read(sess, INITTAB).ok_or("could not read /etc/inittab")?;
    let edited = ensure_stock_inittab_text(&input);
    if dry_run {
        print!("{edited}");
        return Ok(());
    }
    let tmp = "/tmp/inittab.agent-cli-new";
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
    let magik_main_prefix = format!("::sysinit:{}", installed_layout::paths(Layout::Public).main);
    let magik_boot = format!(
        "::sysinit:{}/boot.sh",
        installed_layout::paths(Layout::Public).root
    );
    for raw in input.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with("::sysinit:/media/fat/MiSTer ") && line.ends_with('&') {
            if !wrote {
                out.push("::sysinit:/media/fat/MiSTer &".to_string());
                wrote = true;
            }
            continue;
        }
        if line.starts_with(&magik_main_prefix) || line.starts_with(&magik_boot) {
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
        IniEdit::MenuOutput(profile) => {
            let (direct_video, menu_pal, forced_scandoubler) = profile.settings();
            document.set("Menu", "direct_video", direct_video);
            document.set("Menu", "menu_pal", menu_pal);
            document.set("Menu", "forced_scandoubler", forced_scandoubler);
        }
        IniEdit::SelectMain(value) => {
            document.set("MiSTer", "main", &value);
        }
        IniEdit::MenuMode(mode) => {
            document.set("Menu", "video_mode", &mode);
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
    fn catalog_screenshot_tsv_filters_process_logs_but_keeps_summary_free() {
        let output = concat!(
            "mister-magik-fb [catalog-screenshot-audit]\n",
            "ordinal\ttitle\tpreview_asset_key\tpreview_archive_path\thas_preview\tlaunch_ref\n",
            "0\tGame\tmame-software__nes__game\t/media/fat/assets/nes.mmlz4b\t1\t/game\n",
        );
        assert_eq!(
            catalog_screenshot_tsv(output).unwrap(),
            concat!(
                "ordinal\ttitle\tpreview_asset_key\tpreview_archive_path\thas_preview\tlaunch_ref\n",
                "0\tGame\tmame-software__nes__game\t/media/fat/assets/nes.mmlz4b\t1\t/game\n",
            )
        );
    }

    #[test]
    fn catalog_screenshot_tsv_rejects_missing_runtime_report() {
        assert!(catalog_screenshot_tsv("mister-magik-fb [catalog-screenshot-audit]\n").is_err());
    }

    #[test]
    fn neogeo_setname_reads_structured_and_direct_launches() {
        assert_eq!(
            neogeo_setname("magik-plan:archive:/media/fat/games/NEOGEO/Metal Slug 3 (mslug3).neo")
                .as_deref(),
            Some("mslug3")
        );
        assert_eq!(
            neogeo_setname("/media/fat/_Console/NeoGeo/Metal Slug.mgl").as_deref(),
            Some("metal slug")
        );
    }

    #[test]
    fn neogeo_smoke_matrix_requires_both_memory_classes_and_mgl() {
        let entries = vec![
            (
                "magik-plan:archive:/media/fat/games/NEOGEO/Metal Slug 3 (mslug3).neo".to_string(),
                1,
            ),
            (
                "magik-plan:archive:/media/fat/games/NEOGEO/Garou (garou).neo".to_string(),
                2,
            ),
            (
                "magik-plan:archive:/media/fat/games/NEOGEO/Metal Slug (mslug).neo".to_string(),
                3,
            ),
            ("/media/fat/_Console/NeoGeo/Metal Slug.mgl".to_string(), 4),
        ];
        let matrix = neogeo_smoke_matrix(&entries).unwrap();
        assert_eq!(matrix.len(), 4);
        assert_eq!(matrix[0].setname, "mslug3");
        assert_eq!(matrix[1].setname, "garou");
        assert_eq!(matrix[2].setname, "mslug");
        assert_eq!(matrix[3].role, "direct-mgl");

        assert!(neogeo_smoke_matrix(&entries[..3]).is_err());
        assert!(neogeo_smoke_matrix(&entries[1..]).is_err());
    }

    fn ready_arcade_capture_status() -> Value {
        json!({
            "screen": "arcade",
            "composition_state": "mixed-arcade",
            "crt_font_experiment": "baseline",
            "selected_game_id": "1941",
            "selected_game_has_preview": true,
            "preview_cache_state": "exact",
            "preview_presentation_state": "visible",
        })
    }

    #[test]
    fn first_arcade_capture_waits_for_confirmed_exact_preview() {
        let ready = ready_arcade_capture_status();
        assert!(first_arcade_capture_ready(&ready, None));

        for (field, value) in [
            ("selected_game_has_preview", json!(false)),
            ("preview_cache_state", json!("empty")),
            ("preview_presentation_state", json!("animating")),
        ] {
            let mut pending = ready.clone();
            pending[field] = value;
            assert!(!first_arcade_capture_ready(&pending, None), "{field}");
        }
    }

    #[test]
    fn first_arcade_capture_requires_the_requested_experiment() {
        let mut status = ready_arcade_capture_status();
        assert!(!first_arcade_capture_ready(&status, Some("phase-even")));
        status["crt_font_experiment"] = json!("phase-even");
        assert!(first_arcade_capture_ready(&status, Some("phase-even")));
    }

    #[test]
    fn launch_return_once_starts_at_home_without_automatic_relaunch() {
        let environment = launch_return_once_initial_env();
        assert!(environment.contains(&("MISTER_ARCADE_SELECTED_INDEX".into(), "0".into())));
        assert!(!environment.iter().any(|(key, _)| matches!(
            key.as_str(),
            "MISTER_LAUNCHER_START_SCREEN"
                | "MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED"
                | "MISTER_LAUNCHER_INPUT_SCRIPT"
        )));
        assert!(LAUNCH_RETURN_ONCE_GAME.ends_with("1943 Kai Midway Kaisen (Japan).mra"));
        assert_eq!(ATTENDED_LAUNCH_RETURN_GAME_DWELL, Duration::from_secs(2));
        assert_eq!(
            LAUNCH_RETURN_PHYSICAL_CONFIRMATION_INTERVAL,
            Duration::from_secs(1)
        );
        assert_eq!(LAUNCH_RETURN_ONCE_STEP_DEADLINE_MS, 2_000);
        assert_eq!(LAUNCH_RETURN_CYCLES, 2);
    }

    #[test]
    fn attended_launch_return_accepts_only_settled_visible_magik() {
        let output = Path::new("/tmp/launch-return-once-test");
        let mut summary = json!({
            "schema": "mister-magik-launch-return-once-v2",
            "artifact_status": "passed",
            "physical_video_visible": true,
            "usb_video": { "visibility": "visible" },
            "usb_video_effective_visibility": "visible",
            "usb_video_return_confirmation": {
                "schema": "mister-magik-return-physical-confirmation-v2",
                "temporal_luma_grid": crate::capture::TEMPORAL_LUMA_GRID_ID,
                "temporal_luma_corruption_threshold_permille":
                    crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE,
                "captures": [
                    {
                        "capture": { "visibility": "visible" },
                        "identical_to_primary": false,
                        "temporal_luma_delta_permille": 0
                    },
                    {
                        "capture": { "visibility": "visible" },
                        "identical_to_primary": false,
                        "temporal_luma_delta_permille": 0
                    }
                ]
            },
            "restored_selection": { "semantic": {
                "effective_view": "arcade",
                "return_screen": "arcade",
                "selected_game_id": LAUNCH_RETURN_ONCE_GAME,
                "launch_state": "idle",
                "input_enabled": true,
                "navigation_transition_active": false
            }}
        });
        assert!(validate_attended_launch_return_summary(&summary, output).is_ok());

        summary["usb_video_return_confirmation"]["temporal_luma_grid"] =
            json!("16x9-active-area-v1");
        assert!(validate_attended_launch_return_summary(&summary, output).is_err());
        summary["usb_video_return_confirmation"]["temporal_luma_grid"] =
            json!(crate::capture::TEMPORAL_LUMA_GRID_ID);

        summary["restored_selection"]["semantic"]["launch_state"] = json!("launching");
        assert!(validate_attended_launch_return_summary(&summary, output).is_err());

        summary["restored_selection"]["semantic"]["launch_state"] = json!("idle");
        summary["artifact_status"] = json!("failed");
        summary["physical_video_visible"] = json!(false);
        summary["usb_video"]["visibility"] = json!("black");
        assert!(validate_attended_launch_return_summary(&summary, output).is_err());

        summary["artifact_status"] = json!("passed");
        summary["physical_video_visible"] = json!(true);
        summary["usb_video"]["visibility"] = json!("corrupted");
        summary["usb_video_effective_visibility"] = json!("visible");
        summary["usb_video_return_confirmation"]["captures"] = json!([
            {
                "capture": { "visibility": "corrupted" },
                "identical_to_primary": true,
                "temporal_luma_delta_permille": 0
            },
            {
                "capture": { "visibility": "corrupted" },
                "identical_to_primary": true,
                "temporal_luma_delta_permille": 0
            }
        ]);
        assert!(validate_attended_launch_return_summary(&summary, output).is_err());

        summary["usb_video"]["visibility"] = json!("visible");
        summary["usb_video_return_confirmation"]["captures"][0]["capture"]["visibility"] =
            json!("visible");
        summary["usb_video_return_confirmation"]["captures"][1]["capture"]["visibility"] =
            json!("visible");
        summary["usb_video_return_confirmation"]["captures"][0]["temporal_luma_delta_permille"] =
            json!(crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE);
        assert!(validate_attended_launch_return_summary(&summary, output).is_err());
    }

    #[test]
    fn launch_return_temporal_confirmation_fails_closed() {
        use crate::capture::CaptureVisibility::{Black, Corrupted, SignalLost, Visible};

        let confirmation = |visibility, temporal_luma_delta_permille| LaunchReturnUsbConfirmation {
            visibility,
            temporal_luma_delta_permille,
        };

        assert_eq!(launch_return_effective_usb_visibility(Visible, &[]), None);
        assert_eq!(
            launch_return_effective_usb_visibility(Black, &[]),
            Some(Black)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[confirmation(Visible, 0), confirmation(Visible, 0)]
            ),
            Some(Visible)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[confirmation(Visible, 0), confirmation(Black, 0)]
            ),
            Some(Black)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[confirmation(Visible, 0), confirmation(Corrupted, 0)]
            ),
            Some(Corrupted)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[confirmation(Visible, 0), confirmation(SignalLost, 0)]
            ),
            Some(SignalLost)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Corrupted,
                &[confirmation(Corrupted, 0), confirmation(Corrupted, 0)]
            ),
            Some(Corrupted)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Corrupted,
                &[confirmation(Corrupted, 0), confirmation(Corrupted, 0)]
            ),
            Some(Corrupted)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(Corrupted, &[confirmation(Corrupted, 0)]),
            None
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[
                    confirmation(
                        Visible,
                        crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE - 1
                    ),
                    confirmation(Visible, 0)
                ]
            ),
            Some(Visible)
        );
        assert_eq!(
            launch_return_effective_usb_visibility(
                Visible,
                &[
                    confirmation(Visible, crate::capture::TEMPORAL_LUMA_CORRUPTION_PERMILLE),
                    confirmation(Visible, 0)
                ]
            ),
            Some(Corrupted)
        );
    }

    #[test]
    fn launch_return_once_requires_stable_exact_arcade_selection() {
        let arcade = json!({
            "semantic": {
                "effective_view": "arcade",
                "return_screen": "arcade",
                "launch_state": "idle",
                "active_collection_id": "menu:arcade",
                "selected_game_id": LAUNCH_RETURN_ONCE_GAME,
                "selected_index": 2,
            }
        });
        launch_return_once_validate_restored_selection(&arcade, &arcade).unwrap();

        let mut launcher = arcade.clone();
        launcher["semantic"]["effective_view"] = json!("home");
        launcher["semantic"]["return_screen"] = json!("home");
        assert!(launch_return_once_validate_restored_selection(&arcade, &launcher).is_err());
    }

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
        let args = ["--attended", "--out", "/tmp/evidence", "--usb-video"].map(str::to_string);
        assert_eq!(
            parse_display_matrix_args(&args).unwrap(),
            ("/tmp/evidence", true)
        );
        let args = ["--attended", "--out", "/tmp/evidence"].map(str::to_string);
        assert_eq!(
            parse_display_matrix_args(&args).unwrap(),
            ("/tmp/evidence", false)
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
        let mut values = BTreeMap::new();
        values.insert("format".to_owned(), "mister-magik-platform-v3".to_owned());
        values.insert("platform_release".to_owned(), "platform-v0.1".to_owned());
        values.insert("platform_release_number".to_owned(), "1".to_owned());
        values.insert("platform_bundle_id".to_owned(), "a".repeat(64));
        values.insert("latch_protocol_version".to_owned(), "5".to_owned());
        values.insert("latch_capability_mask".to_owned(), "0x03ff".to_owned());
        for (name, path) in installed_layout::paths(Layout::Development).components() {
            values.insert(format!("{name}_path"), path.to_owned());
            values.insert(format!("{name}_sha256"), "a".repeat(64));
        }
        values.insert("gui_sha256".to_owned(), gui_sha256.to_owned());
        values.insert("platform_contract_sha256".to_owned(), "a".repeat(64));
        for field in ["main_revision", "magik_revision", "menu_revision"] {
            values.insert(field.to_owned(), "b".repeat(40));
        }
        values.insert(
            "qualification_candidate_id".to_owned(),
            mister_magik_platform_manifest_contract::qualification_candidate_id(&values),
        );
        mister_magik_platform_manifest_contract::serialize(&values).unwrap()
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
    fn active_gui_selection_routes_dev_and_public_runtime_commands() {
        let dev = json!({
            "launcher_state": "LauncherActive",
            "fpga_owner": "magik",
            "executable_path": "/media/fat/MiSTer_MagiKDev"
        });
        assert_eq!(
            active_installed_gui_binary(&dev).unwrap(),
            "/media/fat/mister-magik-dev/mister-magik-fb"
        );

        let public = json!({
            "launcher_state": "LauncherActive",
            "fpga_owner": "magik",
            "executable_path": "/media/fat/MiSTer_MagiK"
        });
        assert_eq!(
            active_installed_gui_binary(&public).unwrap(),
            "/media/fat/mister-magik/mister-magik-fb"
        );

        for unavailable in [
            json!({
                "launcher_state": "LauncherSuspended",
                "fpga_owner": "magik",
                "executable_path": "/media/fat/MiSTer_MagiKDev"
            }),
            json!({
                "launcher_state": "LauncherActive",
                "fpga_owner": "main",
                "executable_path": "/media/fat/MiSTer_MagiKDev"
            }),
        ] {
            assert!(active_installed_gui_binary(&unavailable).is_err());
        }
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
    fn shell_quote_handles_single_quotes() {
        assert_eq!(sh("/tmp/simple"), "'/tmp/simple'");
        assert_eq!(sh("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn installed_fpga_metadata_identifies_the_expected_observer() {
        assert_eq!(
            expected_fpga_architecture(
                "rbf_sha256=ignored\ndiagnostic_architecture=scaler-off-domain-scheduler-terminal-v6\n"
            )
            .unwrap(),
            PATCHED_DIAGNOSTIC_ARCHITECTURE
        );
        assert_eq!(
            expected_fpga_architecture(&format!(
                "rbf_sha256={PLATFORM_V0_34_SCHEMA14_RBF_SHA256}\n"
            ))
            .unwrap(),
            PATCHED_DIAGNOSTIC_ARCHITECTURE
        );
        assert!(expected_fpga_architecture(&format!("rbf_sha256={}\n", "0".repeat(64))).is_err());
        assert!(
            expected_fpga_architecture(
                "diagnostic_architecture=one\ndiagnostic_architecture=two\nrbf_sha256=ignored\n"
            )
            .is_err()
        );
    }

    #[test]
    fn fpga_activation_assessment_separates_stale_from_not_ready() {
        let stale = assess_fpga_evidence(
            PATCHED_DIAGNOSTIC_ARCHITECTURE,
            &json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "raw-scaler-boundary-v1",
                "classification": "raw_scaler_active"
            }),
        );
        assert!(matches!(&stale, FpgaActivationAssessment::Stale { .. }));
        assert!(stale.reason().contains("diagnostic_architecture"));

        let not_ready = assess_fpga_evidence(
            PATCHED_DIAGNOSTIC_ARCHITECTURE,
            &json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "unverified-observer-fallback-v1",
                "passive_observer_probe_error": "unsupported",
                "coherence": {"three_samples_valid": false},
                "scaler_fetch_liveness_state": {"raw_samples": [[1], [2], [3]]}
            }),
        );
        assert!(matches!(
            &not_ready,
            FpgaActivationAssessment::NotReady { .. }
        ));
        assert!(not_ready.reason().contains("coherence"));
        assert!(not_ready.reason().contains("raw_samples=[[1],[2],[3]]"));

        let unavailable = assess_fpga_evidence(
            PATCHED_DIAGNOSTIC_ARCHITECTURE,
            &json!({
                "schema": "mister-magik-fpga-video-diagnostics-v1",
                "available": false,
                "coherent": false,
                "classification": "unclassified",
                "reason": "diagnostic readout requires stable LauncherActive ownership"
            }),
        );
        assert!(matches!(
            &unavailable,
            FpgaActivationAssessment::NotReady { .. }
        ));
        assert!(!unavailable.reloadable_not_ready());
        assert!(
            unavailable
                .reason()
                .contains("diagnostic readout requires stable LauncherActive ownership")
        );
    }

    #[test]
    fn fallback_readiness_can_be_promoted_to_one_reloadable_stale_state() {
        let fallback = FpgaActivationAssessment::NotReady {
            expected: PATCHED_DIAGNOSTIC_ARCHITECTURE.into(),
            observed: "unverified-observer-fallback-v1".into(),
            failures: Vec::new(),
        };
        assert!(fallback.reloadable_not_ready());
        assert!(matches!(
            fallback.into_stale(),
            FpgaActivationAssessment::Stale { .. }
        ));
    }

    #[test]
    fn fpga_readiness_policy_reloads_only_definite_or_stable_safe_not_ready() {
        let stale = FpgaActivationAssessment::Stale {
            expected: "patched".into(),
            observed: "stock".into(),
            failures: Vec::new(),
        };
        assert_eq!(
            fpga_readiness_action(&stale, 1, Duration::from_millis(1)),
            FpgaReadinessAction::Reload
        );
        let unavailable = FpgaActivationAssessment::NotReady {
            expected: "patched".into(),
            observed: "unavailable".into(),
            failures: Vec::new(),
        };
        assert_eq!(
            fpga_readiness_action(&unavailable, 3, Duration::from_millis(500)),
            FpgaReadinessAction::Continue
        );
        assert_eq!(
            fpga_readiness_action(&unavailable, 100, FPGA_READINESS_TIMEOUT),
            FpgaReadinessAction::Fail
        );
        let fallback = FpgaActivationAssessment::NotReady {
            expected: "patched".into(),
            observed: "unverified-observer-fallback-v1".into(),
            failures: Vec::new(),
        };
        assert_eq!(
            fpga_readiness_action(&fallback, 3, Duration::from_millis(500)),
            FpgaReadinessAction::Reload
        );
        let coherence_timeout = FpgaActivationAssessment::NotReady {
            expected: "patched".into(),
            observed: "patched".into(),
            failures: vec![FpgaCheckFailure {
                check: "coherence".into(),
                expected: "current".into(),
                actual: "scaler_fetch_liveness_evidence_inconclusive".into(),
            }],
        };
        assert_eq!(
            fpga_readiness_action(&coherence_timeout, 2, Duration::from_millis(500)),
            FpgaReadinessAction::Continue
        );
        assert_eq!(
            fpga_readiness_action(&coherence_timeout, 3, Duration::from_millis(500)),
            FpgaReadinessAction::Continue
        );
        assert_eq!(
            fpga_readiness_action(&coherence_timeout, 3, FPGA_READINESS_TIMEOUT),
            FpgaReadinessAction::Reload
        );
        let ambiguous_same_identity = FpgaActivationAssessment::NotReady {
            expected: "patched".into(),
            observed: "patched".into(),
            failures: Vec::new(),
        };
        assert_eq!(
            fpga_readiness_action(&ambiguous_same_identity, 100, FPGA_READINESS_TIMEOUT),
            FpgaReadinessAction::Fail
        );
        let artifact = FpgaActivationAssessment::ArtifactInvalid {
            detail: "metadata missing".into(),
        };
        assert_eq!(
            fpga_readiness_action(&artifact, 0, Duration::ZERO),
            FpgaReadinessAction::Fail
        );
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
        let main_swap = swap.rfind("MiSTer_MagiKDev'.upload").unwrap();
        let manifest_swap = swap.rfind("platform-v3.manifest'.upload").unwrap();
        assert!(main_swap < manifest_swap);
        assert!(swap.contains("chmod 755"));
        assert!(swap.find("activating").unwrap() < main_swap);

        let rollback = local_main_rollback_script();
        let main_rollback = rollback.find("MiSTer_MagiKDev'.delivery-rollback").unwrap();
        let manifest_rollback = rollback
            .find("platform-v3.manifest'.delivery-rollback")
            .unwrap();
        assert!(rollback.contains("cp -p"));
        assert!(rollback.contains("rolled-back"));
        assert!(main_rollback < manifest_rollback);

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
    fn experimental_fpga_validation_requires_valid_signoff() {
        let root = temp_path("experimental-fpga");
        let patched = root.join("signoff/patched");
        fs::create_dir_all(&patched).unwrap();
        let rbf = patched.join("menu-magik-vblank-latch.rbf");
        let metadata = patched.join("menu-magik-vblank-latch.metadata.txt");
        let report = root.join("signoff/quartus-delta-signoff.tsv");
        fs::write(&rbf, b"diagnostic rbf").unwrap();
        let rbf_sha256 = file_sha256(rbf.clone()).unwrap();
        fs::write(
            &metadata,
            format!(
                "format=mister-magik-fpga-release-v2\nplatform_contract_sha256={}\nsource_commit={}\napply_patch=1\nquartus_mode=local\nrbf_sha256={rbf_sha256}\n",
                "a".repeat(64),
                "b".repeat(40),
            ),
        )
        .unwrap();
        let clean_alm_failure = "quartus_delta_signoff_tsv\tvalid=0\tinvalid_reason=logic_alms_delta\tpatched_setup_slack_min=0.331\tpatched_hold_slack_min=0.241\tpatched_tns_max_abs=0.0\tcustom_sync_seen=1\tcustom_sync_mtbf=1\n";
        fs::write(&report, clean_alm_failure).unwrap();
        assert!(validate_experimental_fpga_inputs(&rbf, &metadata, &report).is_err());

        fs::write(
            &report,
            clean_alm_failure
                .replace("valid=0", "valid=1")
                .replace("invalid_reason=logic_alms_delta", "invalid_reason=ok"),
        )
        .unwrap();
        assert!(validate_experimental_fpga_inputs(&rbf, &metadata, &report).is_ok());

        fs::write(
            &report,
            clean_alm_failure.replace("logic_alms_delta", "patched_setup_slack_min"),
        )
        .unwrap();
        assert!(validate_experimental_fpga_inputs(&rbf, &metadata, &report).is_err());

        fs::write(
            &report,
            clean_alm_failure.replace(
                "patched_hold_slack_min=0.241",
                "patched_hold_slack_min=0.199",
            ),
        )
        .unwrap();
        assert!(validate_experimental_fpga_inputs(&rbf, &metadata, &report).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn experimental_fpga_manifest_changes_only_fpga_identity() {
        let installed = parse_local_main_manifest_text(&local_main_manifest_for(
            &"a".repeat(64),
            &"c".repeat(64),
        ))
        .unwrap();
        let rbf_sha256 = "d".repeat(64);
        let metadata_sha256 = "e".repeat(64);
        let menu_revision = "f".repeat(40);
        let text =
            experimental_fpga_manifest(&installed, &rbf_sha256, &metadata_sha256, &menu_revision)
                .unwrap();
        let candidate = parse_local_main_manifest_text(&text).unwrap();
        for field in RUNTIME_MANIFEST_FIELDS {
            if matches!(
                *field,
                "latch_rbf_sha256"
                    | "latch_metadata_sha256"
                    | "menu_revision"
                    | "qualification_candidate_id"
            ) {
                continue;
            }
            assert_eq!(candidate[*field], installed[*field], "field {field}");
        }
        assert_eq!(candidate["latch_rbf_sha256"], rbf_sha256);
        assert_eq!(candidate["latch_metadata_sha256"], metadata_sha256);
        assert_eq!(candidate["menu_revision"], menu_revision);
        assert_eq!(
            candidate["qualification_candidate_id"],
            local_main_candidate_id(&candidate)
        );
    }

    #[test]
    fn experimental_fpga_activation_requires_exact_current_evidence() {
        let current = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "scaler-completion-repair-v1",
            "available": true,
            "coherent": true,
            "classification": "repair_transport_ready",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": false,
                "protocol_version": 5,
                "flags": 0x03ff,
                "crc": 0,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "presentation_telemetry": {
                "presented_vblank_count": 2,
                "active_sequence": 7,
                "magik_ownership": true,
                "lifetime_invariant_valid": true,
                "crc": 0,
            },
        });
        assert!(experimental_fpga_evidence_is_current(&current));
        assert!(experimental_agent_preload_evidence_accepted(&current));
        for (pointer, value) in [
            ("/diagnostic_architecture", json!("raw-scaler-boundary-v1")),
            ("/available", json!(false)),
            ("/coherent", json!(false)),
            ("/classification", json!("unclassified")),
            ("/sink_visibility", json!("observed")),
            ("/capabilities/passive_video_observer", json!(true)),
        ] {
            let mut invalid = current.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(!experimental_agent_preload_evidence_accepted(&invalid));
        }
        let raw_scaler = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "raw-scaler-boundary-v1",
            "available": true,
            "coherent": true,
            "classification": "raw_scaler_active",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "frame_deltas": [1, 2],
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_scheduler_state": false,
                "raw_scaler_boundary": true,
                "pixel_observer": true,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "raw_scaler_state": {
                "raw_samples": [[1, 2, 3], [1, 2, 3], [1, 2, 3]],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(&raw_scaler));
        assert!(experimental_agent_preload_evidence_accepted(&raw_scaler));
        let frame_integrity = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "raw-scaler-frame-integrity-v1",
            "available": true,
            "coherent": true,
            "classification": "raw_control_stable_since_baseline",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "records_identical": true,
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_scheduler_state": false,
                "raw_scaler_frame_integrity": true,
                "pixel_observer": false,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "raw_scaler_state": {
                "raw_samples": [
                    [3, 7, 0x1234, 0, 0],
                    [3, 7, 0x1234, 0, 0],
                    [3, 7, 0x1234, 0, 0],
                ],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(&frame_integrity));
        assert!(experimental_agent_preload_evidence_accepted(
            &frame_integrity
        ));
        assert!(experimental_fpga_evidence_is_current(&frame_integrity));
        let mut latched_mismatch = frame_integrity.clone();
        latched_mismatch["classification"] = json!("raw_control_mismatch_latched");
        assert!(!experimental_fpga_evidence_is_current(&latched_mismatch));
        let ordered_signature = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "raw-scaler-ordered-signature-v3",
            "available": true,
            "coherent": true,
            "classification": "raw_scaler_ordered_stable",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "classification_stable": true,
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_scheduler_state": false,
                "scaler_pipeline_state": false,
                "scaler_copy_retirement": false,
                "raw_scaler_ordered_signature": true,
                "pixel_observer": true,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "raw_scaler_state": {
                "frame_sequence": [100, 101, 103],
                "ordered_signature": ["5678", "5678", "5678"],
                "raw_samples": vec![vec![10; 5]; 3],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(
            &ordered_signature
        ));
        assert!(experimental_agent_preload_evidence_accepted(
            &ordered_signature
        ));
        assert!(experimental_fpga_evidence_is_current(&ordered_signature));
        let mut ordered_nonadvancing = ordered_signature.clone();
        ordered_nonadvancing["raw_scaler_state"]["frame_sequence"] = json!([100, 100, 103]);
        assert!(!experimental_fpga_evidence_is_current(
            &ordered_nonadvancing
        ));
        let mut ordered_changed = ordered_signature.clone();
        ordered_changed["classification"] =
            json!("raw_scaler_order_changed_requires_static_source_proof");
        assert!(experimental_fpga_evidence_is_current(&ordered_changed));
        let mut ordered_inconclusive = ordered_signature.clone();
        ordered_inconclusive["classification"] = json!("raw_scaler_ordered_evidence_inconclusive");
        assert!(!experimental_fpga_evidence_is_current(
            &ordered_inconclusive
        ));
        let scaler_fetch_liveness = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "scaler-output-scheduler-gates-v1",
            "available": true,
            "coherent": true,
            "classification": "scaler_output_copy_terminal_condition_stall",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "publication_sequence_advancing": true,
                "classification_stable": true,
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_scheduler_state": true,
                "scaler_fetch_liveness": true,
                "scaler_fetch_ordered_signature": false,
                "raw_scaler_ordered_signature": false,
                "pixel_observer": false,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "scaler_fetch_liveness_state": {
                "record_valid": [true, true, true],
                "observer_fault": [false, false, false],
                "publication_sequence": [10, 11, 12],
                "frozen_address_fold": [3, 3, 3],
                "frozen_cause": [1, 1, 1],
                "raw_samples": vec![vec![14; 4]; 3],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(
            &scaler_fetch_liveness
        ));
        assert!(experimental_agent_preload_evidence_accepted(
            &scaler_fetch_liveness
        ));
        assert!(experimental_fpga_evidence_is_current(
            &scaler_fetch_liveness
        ));
        let mut pre_read_liveness = scaler_fetch_liveness.clone();
        pre_read_liveness["diagnostic_architecture"] =
            json!("scaler-off-domain-scheduler-terminal-v6");
        pre_read_liveness["classification"] = json!("scaler_pre_read_request_boundary_stuck");
        pre_read_liveness["capabilities"]["scaler_pre_read_scheduler_evidence"] = json!(true);
        assert!(experimental_fpga_evidence_is_current(&pre_read_liveness));
        let mut observer_self_fault = pre_read_liveness.clone();
        observer_self_fault["diagnostic_architecture"] =
            json!("scaler-off-domain-scheduler-terminal-v4");
        observer_self_fault["coherent"] = json!(false);
        observer_self_fault["classification"] =
            json!("scaler_fetch_liveness_evidence_inconclusive");
        observer_self_fault["coherence"]["publication_sequence_advancing"] = json!(false);
        observer_self_fault["coherence"]["publication_coherent"] = json!(true);
        observer_self_fault["coherence"]["terminal_record_identical"] = json!(true);
        observer_self_fault["coherence"]["classification_stable"] = json!(false);
        observer_self_fault["scaler_fetch_liveness_state"]["raw_samples"] =
            json!([[21, 9, 6, 60722], [21, 9, 6, 60722], [21, 9, 6, 60722]]);
        assert!(experimental_fpga_observer_fault_is_operationally_current(
            &observer_self_fault
        ));
        assert!(!experimental_fpga_evidence_is_current(&observer_self_fault));
        assert!(matches!(
            assess_fpga_evidence(
                "scaler-off-domain-scheduler-terminal-v4",
                &observer_self_fault
            ),
            FpgaActivationAssessment::Current {
                warning: Some(_),
                ..
            }
        ));
        let mut attributed_observer_fault = observer_self_fault.clone();
        attributed_observer_fault["diagnostic_architecture"] =
            json!("scaler-off-domain-scheduler-terminal-v6");
        attributed_observer_fault["scaler_fetch_liveness_state"]["raw_samples"] =
            json!([[23, 9, 3, 16243], [23, 9, 3, 16243], [23, 9, 3, 16243]]);
        assert!(experimental_fpga_observer_fault_is_operationally_current(
            &attributed_observer_fault
        ));
        assert!(matches!(
            assess_fpga_evidence(
                "scaler-off-domain-scheduler-terminal-v6",
                &attributed_observer_fault
            ),
            FpgaActivationAssessment::Current {
                warning: Some(_),
                ..
            }
        ));
        let mut changing_fault = observer_self_fault.clone();
        changing_fault["scaler_fetch_liveness_state"]["raw_samples"][2][2] = json!(7);
        assert!(!experimental_fpga_observer_fault_is_operationally_current(
            &changing_fault
        ));
        let mut unowned_fault = observer_self_fault.clone();
        unowned_fault["coherence"]["latch_ownership_stable"] = json!(false);
        assert!(!experimental_fpga_observer_fault_is_operationally_current(
            &unowned_fault
        ));
        let mut invalid_fault = observer_self_fault.clone();
        invalid_fault["scaler_fetch_liveness_state"]["raw_samples"] =
            json!([[21, 8, 6, 60722], [21, 8, 6, 60722], [21, 8, 6, 60722]]);
        assert!(!experimental_fpga_observer_fault_is_operationally_current(
            &invalid_fault
        ));
        let mut liveness_fault = scaler_fetch_liveness.clone();
        liveness_fault["scaler_fetch_liveness_state"]["observer_fault"] =
            json!([false, true, false]);
        assert!(!experimental_fpga_evidence_is_current(&liveness_fault));
        assert!(!experimental_agent_preload_evidence_accepted(
            &liveness_fault
        ));
        let mut liveness_bootstrap = scaler_fetch_liveness.clone();
        liveness_bootstrap["coherent"] = json!(false);
        liveness_bootstrap["classification"] = json!("scaler_fetch_liveness_evidence_inconclusive");
        liveness_bootstrap["coherence"]["three_samples_valid"] = json!(false);
        liveness_bootstrap["coherence"]["classification_stable"] = json!(false);
        liveness_bootstrap["scaler_fetch_liveness_state"]["record_valid"] =
            json!([false, true, true]);
        assert!(scaler_fetch_liveness_preload_evidence_available(
            &liveness_bootstrap
        ));
        assert!(experimental_agent_preload_evidence_accepted(
            &liveness_bootstrap
        ));
        assert!(!experimental_fpga_evidence_is_current(&liveness_bootstrap));
        liveness_bootstrap["scaler_fetch_liveness_state"]["record_valid"] =
            json!([false, false, false]);
        assert!(!experimental_agent_preload_evidence_accepted(
            &liveness_bootstrap
        ));

        let scaler_fetch_signature = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "scaler-fetch-ordered-signature-v1",
            "available": true,
            "coherent": true,
            "classification": "scaler_fetch_ordered_stable",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "classification_stable": true,
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_fetch_ordered_signature": true,
                "raw_scaler_ordered_signature": false,
                "pixel_observer": false,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "scaler_fetch_state": {
                "capture_sequence": [100, 101, 103],
                "ordered_signature": ["5678", "5678", "5678"],
                "fault_flags": [0, 0, 0],
                "raw_samples": vec![vec![11; 5]; 3],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(
            &scaler_fetch_signature
        ));
        assert!(experimental_agent_preload_evidence_accepted(
            &scaler_fetch_signature
        ));
        assert!(experimental_fpga_evidence_is_current(
            &scaler_fetch_signature
        ));
        let mut fetch_changed = scaler_fetch_signature.clone();
        fetch_changed["classification"] =
            json!("scaler_fetch_order_changed_requires_static_source_proof");
        assert!(experimental_fpga_evidence_is_current(&fetch_changed));
        let mut fetch_fault = scaler_fetch_signature.clone();
        fetch_fault["scaler_fetch_state"]["fault_flags"] = json!([0, 2, 0]);
        assert!(!experimental_fpga_evidence_is_current(&fetch_fault));
        let mut fetch_nonadvancing = scaler_fetch_signature.clone();
        fetch_nonadvancing["scaler_fetch_state"]["capture_sequence"] = json!([100, 100, 103]);
        assert!(!experimental_fpga_evidence_is_current(&fetch_nonadvancing));
        let mut fetch_inconclusive = scaler_fetch_signature.clone();
        fetch_inconclusive["classification"] = json!("scaler_fetch_ordered_evidence_inconclusive");
        assert!(!experimental_fpga_evidence_is_current(&fetch_inconclusive));
        let scaler_copy_retirement = json!({
            "schema": "mister-magik-fpga-video-diagnostics-v2",
            "diagnostic_architecture": "scaler-copy-retirement-v1",
            "available": true,
            "coherent": true,
            "classification": "scaler_copy_retirement_active",
            "sink_visibility": "unobserved",
            "owner_epoch_before": 13,
            "owner_epoch_after": 13,
            "coherence": {
                "three_samples_valid": true,
                "classification_stable": true,
                "latch_ownership_stable": true,
                "launcher_state_stable": true,
                "ownership_check_error": null,
            },
            "capabilities": {
                "passive_video_observer": true,
                "scaler_scheduler_state": false,
                "scaler_pipeline_state": false,
                "scaler_copy_retirement": true,
                "pixel_observer": true,
                "pll_observer": false,
            },
            "latch_status": {
                "active_sequence": 7,
                "flags": 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP,
                "active_width": 960,
                "active_height": 540,
                "active_stride": 1920,
                "crc": 0,
            },
            "raw_scaler_state": {
                "raw_samples": [
                    [6, 0x8867, 0x002a, 0],
                    [6, 0x8867, 0x003a, 0],
                    [6, 0x8867, 0x006a, 0],
                ],
            },
        });
        assert!(experimental_raw_scaler_evidence_available(
            &scaler_copy_retirement
        ));
        assert!(experimental_agent_preload_evidence_accepted(
            &scaler_copy_retirement
        ));
        assert!(experimental_fpga_evidence_is_current(
            &scaler_copy_retirement
        ));
        for rejected in [
            "scaler_copy_lev_dec_missing",
            "scaler_copy_terminal_condition_stall",
            "scaler_copy_metadata_or_buffer_repetition",
            "scaler_copy_buffer_selection_zero",
            "scaler_copy_retirement_evidence_inconclusive",
        ] {
            let mut evidence = scaler_copy_retirement.clone();
            evidence["classification"] = json!(rejected);
            assert!(!experimental_raw_scaler_evidence_available(&evidence));
            assert!(!experimental_agent_preload_evidence_accepted(&evidence));
            assert!(!experimental_fpga_evidence_is_current(&evidence));
        }
        for (pointer, value) in [
            ("/coherence/three_samples_valid", json!(false)),
            ("/coherence/classification_stable", json!(false)),
            ("/capabilities/passive_video_observer", json!(false)),
            ("/capabilities/scaler_scheduler_state", json!(true)),
            ("/capabilities/scaler_pipeline_state", json!(true)),
            ("/capabilities/scaler_copy_retirement", json!(false)),
            ("/capabilities/pixel_observer", json!(false)),
            ("/capabilities/pll_observer", json!(true)),
        ] {
            let mut evidence = scaler_copy_retirement.clone();
            *evidence.pointer_mut(pointer).unwrap() = value;
            assert!(!experimental_fpga_evidence_is_current(&evidence));
        }
        let retired_scheduler = json!({
            "available": false,
            "classification": "unclassified",
            "coherent": false,
            "reason": "read passive FPGA video diagnostics: unsupported raw scaler state schema 1",
            "schema": "mister-magik-fpga-video-diagnostics-v1",
        });
        assert!(experimental_agent_preload_evidence_accepted(
            &retired_scheduler
        ));
        let retired_raw_activity = json!({
            "available": false,
            "classification": "unclassified",
            "coherent": false,
            "reason": "read passive FPGA video diagnostics: unsupported raw scaler state schema 2",
            "schema": "mister-magik-fpga-video-diagnostics-v1",
        });
        assert!(experimental_agent_preload_evidence_accepted(
            &retired_raw_activity
        ));
        let retired_frame_integrity = json!({
            "available": false,
            "classification": "unclassified",
            "coherent": false,
            "reason": "read passive FPGA video diagnostics: unsupported raw scaler state schema 3",
            "schema": "mister-magik-fpga-video-diagnostics-v1",
        });
        assert!(experimental_agent_preload_evidence_accepted(
            &retired_frame_integrity
        ));
        let retired_raw_rgb = json!({
            "available": false,
            "classification": "unclassified",
            "coherent": false,
            "reason": "read passive FPGA video diagnostics: unsupported raw scaler state schema 4",
            "schema": "mister-magik-fpga-video-diagnostics-v1",
        });
        assert!(experimental_agent_preload_evidence_accepted(
            &retired_raw_rgb
        ));
        for (pointer, value) in [
            ("/schema", json!("mister-magik-fpga-video-diagnostics-v2")),
            ("/available", json!(true)),
            ("/coherent", json!(true)),
            ("/classification", json!("raw_scaler_active")),
            ("/reason", json!("unsupported diagnostic")),
        ] {
            let mut invalid = retired_scheduler.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(!experimental_agent_preload_evidence_accepted(&invalid));
        }
        for classification in [
            "raw_scaler_timing_stalled",
            "raw_scaler_no_active_video",
            "raw_scaler_black",
            "raw_scaler_sparse_or_corrupt",
            "raw_scaler_active",
        ] {
            let mut classified = raw_scaler.clone();
            classified["classification"] = json!(classification);
            assert!(experimental_fpga_evidence_is_current(&classified));
        }
        for (pointer, value) in [
            ("/coherence/three_samples_valid", json!(false)),
            ("/capabilities/passive_video_observer", json!(false)),
            ("/capabilities/scaler_scheduler_state", json!(true)),
            ("/capabilities/raw_scaler_boundary", json!(false)),
            ("/capabilities/pixel_observer", json!(false)),
            ("/capabilities/pll_observer", json!(true)),
            ("/classification", json!("raw_scaler_evidence_inconclusive")),
            (
                "/raw_scaler_state/raw_samples",
                json!([[1, 2, 3], [1, 2, 3]]),
            ),
        ] {
            let mut invalid = raw_scaler.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(!experimental_fpga_evidence_is_current(&invalid));
        }
        for pointer in [
            "/coherence/latch_ownership_stable",
            "/coherence/launcher_state_stable",
            "/presentation_telemetry/magik_ownership",
            "/presentation_telemetry/lifetime_invariant_valid",
        ] {
            let mut missing_capability = current.clone();
            *missing_capability.pointer_mut(pointer).unwrap() = Value::Bool(false);
            assert!(!experimental_fpga_evidence_is_current(&missing_capability));
        }
        for (pointer, value) in [
            ("/capabilities/passive_video_observer", json!(true)),
            ("/capabilities/protocol_version", json!(4)),
            ("/capabilities/flags", json!(0x01ff)),
            ("/latch_status/active_stride", json!(1919)),
            ("/presentation_telemetry/presented_vblank_count", json!(1)),
            ("/presentation_telemetry/active_sequence", json!(8)),
            ("/owner_epoch_after", json!(14)),
            ("/sink_visibility", json!("observed")),
        ] {
            let mut invalid = current.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(!experimental_fpga_evidence_is_current(&invalid));
        }
        for stale in [
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v1",
                "available": true,
                "coherent": true,
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": false,
                "coherent": true,
                "capabilities": {"physical_hdmi_pll_lock": true},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": false,
                "capabilities": {"physical_hdmi_pll_lock": true},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "retired-wide-observer",
                "available": true,
                "coherent": true,
                "capabilities": {"physical_hdmi_pll_lock": true},
                "hdmi_lock": {"raw_words": [1, 7, 0, 0]},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": true,
                "capabilities": {},
                "hdmi_lock": {"raw_words": [1, 7, 0, 0]},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": true,
                "capabilities": {"physical_hdmi_pll_lock": true},
                "hdmi_lock": {"raw_words": [1, 7, 0]},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": true,
                "capabilities": {"physical_hdmi_pll_lock": true},
                "hdmi_lock": {"raw_words": [1, 7, 0, 0, 0]},
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": true,
                "capabilities": {
                    "physical_hdmi_pll_lock": true,
                    "final_hdmi_output": false,
                },
                "hdmi_lock": {"raw_words": [1, 7, 0, 0]},
                "final_hdmi_output_activity": {
                    "first": {"raw_words": [1, 1, 0, 0, 1, 0]},
                    "second": {"raw_words": [1, 1, 0, 0, 7, 0]},
                },
            }),
            json!({
                "schema": "mister-magik-fpga-video-diagnostics-v2",
                "diagnostic_architecture": "hdmi-lock-evidence-v1",
                "available": true,
                "coherent": true,
                "capabilities": {
                    "physical_hdmi_pll_lock": true,
                    "final_hdmi_output": true,
                },
                "hdmi_lock": {"raw_words": [1, 7, 0, 0]},
                "final_hdmi_output_activity": {
                    "first": {"raw_words": [1, 1, 0, 0, 1]},
                    "second": {"raw_words": [1, 1, 0, 0, 7, 0, 0]},
                },
            }),
        ] {
            assert!(!experimental_fpga_evidence_is_current(&stale));
        }
    }

    #[test]
    fn experimental_fpga_activation_loads_the_exact_installed_dev_rbf() {
        let command =
            acknowledged_main_command(&format!("load_core {EXPERIMENTAL_FPGA_RBF_REMOTE}"));
        assert!(command.contains(&format!("load_core {EXPERIMENTAL_FPGA_RBF_REMOTE}")));
        assert!(!command.contains("/media/fat/menu.rbf"));
        assert!(!command.contains("rbf_load"));
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
        fn timing_lane(&self) -> Option<DeliveryLane> {
            Some(DeliveryLane::Runtime)
        }

        fn snapshot(&mut self) -> std::result::Result<(), DeviceFailure> {
            self.step("snapshot")
        }

        fn deploy(
            &mut self,
            metrics: &mut DeliveryTransferMetrics,
        ) -> std::result::Result<(), DeviceFailure> {
            metrics.files = 3;
            metrics.bytes = 1_024;
            metrics.upload_ms = 8;
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
        let mut timings = Vec::new();
        assert_eq!(
            run_coherent_delivery(&mut actions, false, &mut timings).unwrap(),
            "healthy"
        );
        assert_eq!(
            actions.events,
            ["snapshot", "deploy", "activate", "smoke", "commit"]
        );
        assert!(matches!(
            timings.as_slice(),
            [
                DeliveryTimingSample::Stage {
                    lane: DeliveryLane::Runtime,
                    stage: "snapshot",
                    status: DeliveryTimingStatus::Passed,
                    ..
                },
                DeliveryTimingSample::Transfer {
                    lane: DeliveryLane::Runtime,
                    status: DeliveryTimingStatus::Passed,
                    metrics: DeliveryTransferMetrics {
                        files: 3,
                        bytes: 1_024,
                        upload_ms: 8,
                        ..
                    },
                },
                DeliveryTimingSample::Stage {
                    lane: DeliveryLane::Runtime,
                    stage: "activate",
                    status: DeliveryTimingStatus::Passed,
                    ..
                },
                DeliveryTimingSample::Smoke {
                    lane: DeliveryLane::Runtime,
                    status: DeliveryTimingStatus::Passed,
                    ..
                },
                DeliveryTimingSample::Stage {
                    lane: DeliveryLane::Runtime,
                    stage: "commit",
                    status: DeliveryTimingStatus::Passed,
                    ..
                },
            ]
        ));
    }

    #[test]
    fn coherent_platform_reboots_and_commits_after_smoke() {
        let mut actions = ScriptedCoherentDelivery::default();
        let mut timings = Vec::new();
        run_coherent_delivery(&mut actions, true, &mut timings).unwrap();
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
        let mut runtime_timings = Vec::new();
        assert!(run_coherent_delivery(&mut runtime, false, &mut runtime_timings).is_err());
        assert_eq!(
            runtime.events,
            [
                "snapshot", "deploy", "activate", "smoke", "rollback", "health"
            ]
        );
        assert!(runtime_timings.iter().any(|sample| matches!(
            sample,
            DeliveryTimingSample::Smoke {
                status: DeliveryTimingStatus::Failed,
                ..
            }
        )));

        let mut platform = ScriptedCoherentDelivery {
            fail_at: Some("smoke"),
            ..ScriptedCoherentDelivery::default()
        };
        let mut platform_timings = Vec::new();
        assert!(run_coherent_delivery(&mut platform, true, &mut platform_timings).is_err());
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
        let mut timings = Vec::new();
        assert!(matches!(
            run_coherent_delivery(&mut actions, false, &mut timings),
            Err(DeviceFailure::RecoveryRequired(_))
        ));
        assert!(matches!(
            timings.as_slice(),
            [
                DeliveryTimingSample::Stage {
                    stage: "snapshot",
                    status: DeliveryTimingStatus::Passed,
                    ..
                },
                DeliveryTimingSample::Transfer {
                    status: DeliveryTimingStatus::Failed,
                    metrics: DeliveryTransferMetrics {
                        files: 3,
                        bytes: 1_024,
                        upload_ms: 8,
                        ..
                    },
                    ..
                },
                DeliveryTimingSample::Stage {
                    stage: "rollback",
                    status: DeliveryTimingStatus::Failed,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn coherent_delivery_does_not_rollback_after_commit_cleanup_starts() {
        let mut actions = ScriptedCoherentDelivery {
            fail_at: Some("commit"),
            ..ScriptedCoherentDelivery::default()
        };
        assert!(matches!(
            run_coherent_delivery(&mut actions, false, &mut Vec::new()),
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
        let mut metrics = DeliveryTransferMetrics::default();

        let report = tx
            .run_with(&remote, 0, Instant::now(), &mut metrics)
            .unwrap();
        let events = remote.events();
        assert!(events[2].ends_with("mister-magik-fb.upload"));
        assert!(events[3].ends_with("platform-v3.manifest.upload"));
        assert!(events[4].contains("sha256sum"));
        assert!(
            events[5].find("mister-magik-fb.upload").unwrap()
                < events[5].find("platform-v3.manifest.upload").unwrap()
        );
        assert_eq!(metrics.files, 2);
        assert_eq!(
            metrics.bytes,
            fs::metadata(&local).unwrap().len() + fs::metadata(&manifest).unwrap().len()
        );
        assert_eq!(report.transferred_files, metrics.files);
        assert_eq!(report.transferred_bytes, metrics.bytes);
        assert_eq!(report.transfer_ms, metrics.upload_ms);
        assert_eq!(report.binary_transport, BinaryTransport::Sftp);
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
        let mut metrics = DeliveryTransferMetrics::default();

        assert!(
            tx.run_with(&remote, 0, Instant::now(), &mut metrics)
                .is_err()
        );
        let events = remote.events();

        assert!(events[1].contains("mister_magik_suspend"));
        assert!(events[2].starts_with("put "));
        assert!(events[3].starts_with("rm -f "));
        assert!(events[4].contains("mister_magik_resume"));
        assert_eq!(events.len(), 5);
        assert_eq!(metrics.files, 0);
        assert_eq!(metrics.bytes, 0);
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

        assert!(
            tx.run_with(
                &remote,
                0,
                Instant::now(),
                &mut DeliveryTransferMetrics::default(),
            )
            .is_err()
        );
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

        assert!(
            tx.run_with(
                &remote,
                0,
                Instant::now(),
                &mut DeliveryTransferMetrics::default(),
            )
            .is_err()
        );
        let events = remote.events();

        assert_eq!(events.len(), 2);
        assert!(events[0].contains("mkdir -p"));
        assert!(events[1].starts_with("rm -f "));
        assert!(!events.iter().any(|event| event.contains("suspend")));
        let _ = fs::remove_file(local);
        let _ = fs::remove_file(manifest);
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
            elapsed_ms: 0,
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

        let mut migration = latch.clone();
        migration["input_enabled"] = json!(false);
        migration["catalog_ready"] = json!(true);
        migration["catalog_scan_visible"] = json!(true);
        migration["startup_mode"] = json!("cold_no_catalog");
        migration["startup_reveal_state"] = json!("catalog_progress_visible");
        migration["frames"] = json!(12);
        assert_eq!(
            validate_delivery_present_state(&migration, None).unwrap(),
            DeliveryPresentState::CatalogMigration
        );

        migration["catalog_scan_visible"] = json!(false);
        assert!(validate_delivery_present_state(&migration, None).is_err());

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
    fn delivery_accepts_navigation_states_without_an_allowlist() {
        let mut split =
            delivery_status("system-hub", "system-hub", "fpga-vblank-latch-hidden", "ok");
        split["screen"] = json!("future-screen");
        assert_eq!(
            validate_delivery_present_state(&split, None).unwrap(),
            DeliveryPresentState::Latch
        );
    }

    #[test]
    fn delivery_rejects_nonterminal_recovery() {
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
    fn delivery_rejects_unexplained_fallback() {
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
    fn platform_deploy_validates_every_required_file_and_publishes_manifest_last() {
        let stage = temp_path("platform-stage");
        for (relative, _) in platform_deploy_files() {
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
        for (relative, _) in platform_deploy_files() {
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
        let mut metrics = DeliveryTransferMetrics::default();

        let report = transaction.run_with(&remote, &mut metrics).unwrap();

        assert_eq!(report.changed_files, 0);
        assert_eq!(report.skipped_files, platform_deploy_files().len());
        assert_eq!(report.transferred_bytes, 0);
        assert_eq!(report.transfer_ms, 0);
        assert_eq!(metrics, DeliveryTransferMetrics::default());
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
        let mut metrics = DeliveryTransferMetrics::default();

        let report = transaction.run_with(&remote, &mut metrics).unwrap();
        let events = remote.events();
        let uploads = events
            .iter()
            .filter(|event| event.starts_with("put "))
            .collect::<Vec<_>>();
        let activation = events.last().unwrap();

        assert_eq!(report.changed_files, 2);
        assert_eq!(metrics.files, 2);
        assert_eq!(metrics.bytes, report.transferred_bytes);
        assert_eq!(report.transfer_ms, metrics.upload_ms);
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
        assert!(
            !events.iter().any(|event| {
                event.starts_with("put ") && event.contains("magik-metadata-v1.bin")
            })
        );
        assert!(!activation.contains("cp -p '/media/fat/mister-magik-dev/mame.sqlite3'"));
        assert!(!activation.contains("magik-metadata-v1.bin"));
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

        let report = transaction
            .run_with(&remote, &mut DeliveryTransferMetrics::default())
            .unwrap();

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

        let error = transaction
            .run_with(&remote, &mut DeliveryTransferMetrics::default())
            .unwrap_err()
            .to_string();

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
    fn active_runtime_requires_the_exact_development_launcher_state() {
        let development = parse_active_runtime_status(Some(
            r#"{"executable_path":"/media/fat/MiSTer_MagiKDev","launcher_state":"LauncherActive"}"#,
        ));
        assert!(development.is_development_launcher());
        assert!(!development.is_public_launcher());

        let public = parse_active_runtime_status(Some(
            r#"{"executable_path":"/media/fat/MiSTer_MagiK","launcher_state":"LauncherActive"}"#,
        ));
        assert!(public.is_public_launcher());
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
            assert!(!active.is_public_launcher());
        }
        assert_eq!(
            parse_active_runtime_status(None).description(),
            "executable_path=unknown launcher_state=unknown"
        );
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
        assert!(begin.contains(RELEASE_SNAPSHOT.as_str()));
        assert!(RELEASE_SNAPSHOT.as_str().starts_with("/media/fat/"));
        let rearm = release_rearm_token_command();
        assert!(rearm.contains(RELEASE_TOKEN));
        assert!(!rearm.contains(RELEASE_SNAPSHOT.as_str()));
        let catalog = release_catalog_command();
        assert!(catalog.contains("pidof MiSTer_MagiKDev"));
        assert!(catalog.contains("root='/media/fat/mister-magik-dev'"));
        let recovery = release_recovery_command();
        assert!(recovery.contains(RELEASE_TOKEN));
        assert!(recovery.contains("attended-non-network-recovery-confirmed"));
        let restore = release_restore_command();
        assert!(!restore.contains(";;"));
        assert!(restore.contains(RELEASE_SNAPSHOT.as_str()));
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
        assert!(repair.contains("rm -f '/media/fat/mister-magik/launcher.env'"));
        assert!(repair.contains("/media/fat/mister-magik-dev/launcher.env"));
        assert!(repair.contains("/media/fat/mister-magik-dev/rebuild-on-next-boot"));
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

    #[test]
    fn catalog_snapshots_accept_only_catalog_sqlite_databases() {
        assert!(is_catalog_database_path(
            "/media/fat/mister-magik/catalog-fast-v1/state/catalog-state.sqlite3"
        ));
        assert!(is_catalog_database_path(
            "/media/fat/mister-magik-dev/catalog-fast-v1/systems/arcade/active.sqlite3"
        ));
        assert!(!is_catalog_database_path(
            "/media/fat/mister-magik-dev/catalog-fast-v1/../agent.token"
        ));
        assert!(!is_catalog_database_path("/media/fat/MiSTer.ini"));
    }

    #[test]
    fn runtime_metadata_qualification_requires_compact_integrity() {
        let mut report = json!({
            "schema": "mister-magik-runtime-metadata-qualification-v2",
            "compact": {
                "valid": true,
                "file_bytes": 7_827_977,
                "shard_count": 35,
                "software_rows": 73_853,
                "arcade_mame_rows": 50_368,
                "arcade_hbmame_rows": 9_503,
                "arcade_mister_rows": 3_009
            },
            "legacy_sqlite_absence": runtime_metadata_legacy_sqlite_test_evidence(None),
        });
        assert!(validate_runtime_metadata_qualification(&report).is_ok());

        report["compact"]["shard_count"] = json!(34);
        assert!(validate_runtime_metadata_qualification(&report).is_err());
    }

    #[test]
    fn runtime_metadata_qualification_requires_all_four_legacy_sqlite_paths_absent() {
        let mut report = json!({
            "schema": "mister-magik-runtime-metadata-qualification-v2",
            "compact": {
                "valid": true,
                "file_bytes": 7_827_977,
                "shard_count": 35,
                "software_rows": 73_853,
                "arcade_mame_rows": 50_368,
                "arcade_hbmame_rows": 9_503,
                "arcade_mister_rows": 3_009
            },
            "legacy_sqlite_absence": runtime_metadata_legacy_sqlite_test_evidence(None),
        });
        assert!(validate_runtime_metadata_qualification(&report).is_ok());

        report["legacy_sqlite_absence"] = runtime_metadata_legacy_sqlite_test_evidence(Some(2));
        let error = validate_runtime_metadata_qualification(&report).unwrap_err();
        assert!(error.to_string().contains("legacy SQLite"));

        report["legacy_sqlite_absence"]["paths"][2]["present"] = json!(false);
        report["legacy_sqlite_absence"]["all_absent"] = json!(true);
        assert!(validate_runtime_metadata_qualification(&report).is_ok());

        report["legacy_sqlite_absence"]["paths"][0]["path"] =
            json!("/media/fat/mister-magik/other.sqlite3");
        assert!(validate_runtime_metadata_qualification(&report).is_err());
    }

    fn runtime_metadata_legacy_sqlite_test_evidence(present: Option<usize>) -> Value {
        let paths = LEGACY_RUNTIME_METADATA_PATHS
            .iter()
            .enumerate()
            .map(|(index, path)| json!({"path": path, "present": Some(index) == present}))
            .collect::<Vec<_>>();
        json!({
            "schema": "mister-magik-runtime-metadata-legacy-sqlite-absence-v1",
            "paths": paths,
            "all_absent": present.is_none(),
        })
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
