// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    Status(StatusArgs),
    ArmingStatus,
    Mode {
        #[command(subcommand)]
        command: ModeCommand,
    },
    Scene(SceneArgs),
    Display {
        #[command(subcommand)]
        command: DisplayCommand,
    },
    Crt {
        #[command(subcommand)]
        command: CrtCommand,
    },
    Capture {
        #[command(subcommand)]
        command: CaptureCommand,
    },
    Reboot(AttendedArgs),
    Logs,
    Events,
    Diagnostics(DiagnosticsArgs),
    Launcher {
        #[command(subcommand)]
        command: LauncherCommand,
    },
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    Media {
        #[command(subcommand)]
        command: MediaCommand,
    },
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
pub enum ModeCommand {
    Status,
    Set(ModeSetArgs),
}

#[derive(Debug, Args)]
pub struct ModeSetArgs {
    #[arg(value_enum)]
    mode: DeviceMode,
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DeviceMode {
    Dev,
    Public,
    Stock,
}

#[derive(Debug, Args)]
pub struct SceneArgs {
    #[arg(value_enum)]
    scene: Scene,
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Scene {
    Launcher,
    ControllerTest,
    TearPattern,
    VideoPlayback,
    CrtTrial,
}

#[derive(Debug, Subcommand)]
pub enum DisplayCommand {
    Set(DisplaySetArgs),
    Matrix(DisplayMatrixArgs),
}

#[derive(Debug, Args)]
pub struct DisplaySetArgs {
    #[arg(value_enum)]
    mode: DisplayMode,
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    keep: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DisplayMode {
    Auto,
    Hdmi1280x720p60,
    Hdmi1366x768p60,
    Hdmi1920x1080p60,
    Hdmi1920x1200p60,
    Hdmi2048x1536p60,
    Hdmi2560x1440p60,
    Crt240p60,
    Crt288p50,
    Crt480p60,
    Crt576p50,
}

#[derive(Debug, Args)]
pub struct DisplayMatrixArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    usb_video: bool,
    #[arg(long)]
    screensaver_wait: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub enum CrtCommand {
    Qualify(CrtQualifyArgs),
    Probe(CrtProbeArgs),
    Restore(AttendedArgs),
}

#[derive(Debug, Args)]
pub struct CrtQualifyArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CrtProbeArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pattern: String,
    #[arg(long)]
    seconds: u64,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    Framebuffer(FramebufferArgs),
}

#[derive(Debug, Args)]
pub struct FramebufferArgs {
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct AttendedArgs {
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Args)]
pub struct DiagnosticsArgs {
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum LauncherCommand {
    Status,
    Restart(AttendedArgs),
    ReturnToLauncher(AttendedArgs),
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    Inspect,
    Query(CatalogQueryArgs),
    Cores,
}

#[derive(Debug, Args)]
pub struct CatalogQueryArgs {
    #[arg(long)]
    database: String,
    #[arg(long)]
    sql: String,
}

#[derive(Debug, Subcommand)]
pub enum MediaCommand {
    Check(MediaArgs),
    Download(MediaDownloadArgs),
}

#[derive(Debug, Args)]
pub struct MediaArgs {
    #[arg(long)]
    system: Option<String>,
    #[arg(long)]
    manifest_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct MediaDownloadArgs {
    #[command(flatten)]
    media: MediaArgs,
    #[arg(long, required = true)]
    attended: bool,
}

pub fn run(command: DeviceCommand) -> AgentResult<()> {
    crate::host::run_cli_args(legacy_args(command)).map_err(|error| error.to_string().into())
}

fn legacy_args(command: DeviceCommand) -> Vec<String> {
    match command {
        DeviceCommand::Status(args) => with_flag(strings(["status"]), "--json", args.json),
        DeviceCommand::ArmingStatus => strings(["arming-status"]),
        DeviceCommand::Mode { command } => match command {
            ModeCommand::Status => strings(["mode", "status"]),
            ModeCommand::Set(args) => strings(["mode", args.mode.as_str()]),
        },
        DeviceCommand::Scene(args) => {
            let mut values = strings(["scene", args.scene.as_str()]);
            if let Some(seconds) = args.seconds {
                values.push(seconds.to_string());
            }
            values
        }
        DeviceCommand::Display { command } => match command {
            DisplayCommand::Set(args) => with_flag(
                strings(["display-mode", args.mode.as_str(), "--attended"]),
                "--keep",
                args.keep,
            ),
            DisplayCommand::Matrix(args) => {
                let mut values = strings([
                    "display-matrix",
                    "--attended",
                    "--out",
                    &args.out.to_string_lossy(),
                ]);
                if args.usb_video {
                    values.push("--usb-video".into());
                }
                if let Some(seconds) = args.screensaver_wait {
                    values.extend(["--screensaver-wait".into(), seconds.to_string()]);
                }
                values
            }
        },
        DeviceCommand::Crt { command } => match command {
            CrtCommand::Qualify(args) => {
                let mut values = strings(["crt", "qualify", "--attended"]);
                if let Some(out) = args.out {
                    values.extend(["--out".into(), out.to_string_lossy().into_owned()]);
                }
                values
            }
            CrtCommand::Probe(args) => strings([
                "crt",
                "probe",
                "--attended",
                "--pattern",
                &args.pattern,
                "--seconds",
                &args.seconds.to_string(),
                "--out",
                &args.out.to_string_lossy(),
            ]),
            CrtCommand::Restore(_) => strings(["crt", "qualify", "--restore"]),
        },
        DeviceCommand::Capture { command } => match command {
            CaptureCommand::Framebuffer(args) => {
                let mut values = strings(["--capture-buffer"]);
                if let Some(output) = args.output {
                    values.extend(["--output".into(), output.to_string_lossy().into_owned()]);
                }
                values
            }
        },
        DeviceCommand::Reboot(_) => strings(["agent", "reboot-wait"]),
        DeviceCommand::Logs => strings(["agent", "logs"]),
        DeviceCommand::Events => strings(["agent", "timeline"]),
        DeviceCommand::Diagnostics(args) => {
            strings(["agent", "diagnostics", "--out", &args.out.to_string_lossy()])
        }
        DeviceCommand::Launcher { command } => match command {
            LauncherCommand::Status => strings(["agent", "magik", "status"]),
            LauncherCommand::Restart(_) => strings(["launcher-restart"]),
            LauncherCommand::ReturnToLauncher(_) => {
                strings(["agent", "magik", "return-to-launcher"])
            }
        },
        DeviceCommand::Catalog { command } => match command {
            CatalogCommand::Inspect => strings(["catalog"]),
            CatalogCommand::Query(args) => strings([
                "catalog-query",
                "--database",
                &args.database,
                "--sql",
                &args.sql,
            ]),
            CatalogCommand::Cores => strings(["core-list"]),
        },
        DeviceCommand::Media { command } => match command {
            MediaCommand::Check(args) => media_args("media-check", args),
            MediaCommand::Download(args) => media_args("media-download", args.media),
        },
    }
}

fn media_args(action: &str, args: MediaArgs) -> Vec<String> {
    let mut values = vec![action.to_owned()];
    if let Some(system) = args.system {
        values.extend(["--system".into(), system]);
    }
    if let Some(url) = args.manifest_url {
        values.extend(["--manifest-url".into(), url]);
    }
    values
}

fn with_flag(mut values: Vec<String>, flag: &str, enabled: bool) -> Vec<String> {
    if enabled {
        values.push(flag.to_owned());
    }
    values
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}

impl DeviceMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Public => "public",
            Self::Stock => "stock",
        }
    }
}

impl Scene {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::ControllerTest => "controller_test",
            Self::TearPattern => "tear_pattern",
            Self::VideoPlayback => "video_playback",
            Self::CrtTrial => "crt_trial",
        }
    }
}

impl DisplayMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hdmi1280x720p60 => "hdmi-1280x720p60",
            Self::Hdmi1366x768p60 => "hdmi-1366x768p60",
            Self::Hdmi1920x1080p60 => "hdmi-1920x1080p60",
            Self::Hdmi1920x1200p60 => "hdmi-1920x1200p60",
            Self::Hdmi2048x1536p60 => "hdmi-2048x1536p60",
            Self::Hdmi2560x1440p60 => "hdmi-2560x1440p60",
            Self::Crt240p60 => "crt-240p60",
            Self::Crt288p50 => "crt-288p50",
            Self::Crt480p60 => "crt-480p60",
            Self::Crt576p50 => "crt-576p50",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: DeviceCommand,
    }

    #[test]
    fn mutations_require_attendance_during_parsing() {
        assert!(TestCli::try_parse_from(["test", "mode", "set", "dev"]).is_err());
        assert!(TestCli::try_parse_from(["test", "reboot"]).is_err());
        assert!(TestCli::try_parse_from(["test", "mode", "set", "dev", "--attended"]).is_ok());
    }

    #[test]
    fn retired_generic_operations_are_not_parseable() {
        for command in ["get", "wait", "recover", "doctor", "connected", "agent"] {
            assert!(TestCli::try_parse_from(["test", command]).is_err());
        }
    }
}
