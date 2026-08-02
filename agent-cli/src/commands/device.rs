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
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub enum ModeCommand {
    Status,
    Set(ModeSetArgs),
}

#[derive(Debug, Args)]
pub struct ModeSetArgs {
    #[arg(value_enum)]
    pub(crate) mode: DeviceMode,
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
    pub(crate) scene: Scene,
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pub(crate) seconds: Option<u64>,
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
    pub(crate) mode: DisplayMode,
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pub(crate) keep: bool,
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
    pub(crate) out: PathBuf,
    #[arg(long)]
    pub(crate) usb_video: bool,
    #[arg(long)]
    pub(crate) screensaver_wait: Option<u64>,
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
    pub(crate) out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct CrtProbeArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long)]
    pub(crate) pattern: String,
    #[arg(long)]
    pub(crate) seconds: u64,
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    Framebuffer(FramebufferArgs),
}

#[derive(Debug, Args)]
pub struct FramebufferArgs {
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct AttendedArgs {
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Args)]
pub struct DiagnosticsArgs {
    #[arg(long)]
    pub(crate) out: PathBuf,
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
    pub(crate) database: String,
    #[arg(long)]
    pub(crate) sql: String,
}

#[derive(Debug, Subcommand)]
pub enum MediaCommand {
    Check(MediaArgs),
    Download(MediaDownloadArgs),
}

#[derive(Debug, Args)]
pub struct MediaArgs {
    #[arg(long)]
    pub(crate) system: Option<String>,
    #[arg(long)]
    pub(crate) manifest_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct MediaDownloadArgs {
    #[command(flatten)]
    pub(crate) media: MediaArgs,
    #[arg(long, required = true)]
    attended: bool,
}

pub fn run(command: DeviceCommand) -> AgentResult<()> {
    let mutation = command.is_mutation();
    let mut device = crate::device::DeviceClient::default();
    if mutation {
        device.mutate(|device| device.run_operator(&command))
    } else {
        device.read(|device| device.run_operator(&command))
    }
}

impl DeviceCommand {
    pub(crate) fn is_mutation(&self) -> bool {
        match self {
            Self::Status(_)
            | Self::ArmingStatus
            | Self::Logs
            | Self::Events
            | Self::Diagnostics(_)
            | Self::Capture { .. } => false,
            Self::Mode { command } => matches!(command, ModeCommand::Set(_)),
            Self::Scene(_) | Self::Reboot(_) => true,
            Self::Display { .. } | Self::Crt { .. } => true,
            Self::Launcher { command } => !matches!(command, LauncherCommand::Status),
            Self::Catalog { .. } => false,
            Self::Media { command } => matches!(command, MediaCommand::Download(_)),
        }
    }
}

impl DeviceMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Public => "public",
            Self::Stock => "stock",
        }
    }
}

impl Scene {
    pub(crate) const fn as_str(self) -> &'static str {
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
    pub(crate) const fn as_str(self) -> &'static str {
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

    #[test]
    fn retry_category_is_derived_from_the_typed_command() {
        let status = TestCli::try_parse_from(["test", "status"]).unwrap();
        assert!(!status.command.is_mutation());
        let reboot = TestCli::try_parse_from(["test", "reboot", "--attended"]).unwrap();
        assert!(reboot.command.is_mutation());
    }
}
