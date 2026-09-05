// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    Status(StatusArgs),
    ArmingStatus,
    TransferCheck(TransferCheckArgs),
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
    Fpga {
        #[command(subcommand)]
        command: DeviceFpgaCommand,
    },
}

#[derive(Debug, Args)]
pub struct TransferCheckArgs {
    #[arg(long)]
    pub(crate) artifact: PathBuf,
    #[arg(long)]
    pub(crate) fetch_installed: bool,
    #[arg(long, required = true)]
    attended: bool,
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
    RouteStatus,
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
    #[arg(long, value_name = "STEM")]
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
    Restart(LauncherRestartArgs),
    CaptureFirstArcade(FirstArcadeCaptureArgs),
    LaunchReturnOnce(FirstArcadeCaptureArgs),
    VerifyNeogeoSdram(FirstArcadeCaptureArgs),
    CaptureCrtFontAb(CrtFontAbCaptureArgs),
    CaptureSnesHub(FirstArcadeCaptureArgs),
    ReturnToLauncher(AttendedArgs),
}

#[derive(Debug, Args)]
pub struct LauncherRestartArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long, value_enum)]
    pub(crate) crt_font_experiment: Option<CrtFontExperiment>,
    #[arg(long, value_enum)]
    pub(crate) crt240_composition: Option<Crt240Composition>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CrtFontExperiment {
    PhaseEven,
    CoverageMax,
    DominantRow,
    Xerxes,
    XerxesPerfect,
    YesterdayPerfect,
    Bacteria,
    BacteriaHalf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Crt240Composition {
    Native,
    #[value(name = "legacy-480")]
    Legacy480,
}

impl Crt240Composition {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Legacy480 => "legacy-480",
        }
    }
}

#[derive(Debug, Args)]
pub struct FirstArcadeCaptureArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long, value_name = "STEM")]
    pub(crate) output: PathBuf,
}

#[derive(Debug, Args)]
pub struct CrtFontAbCaptureArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long, value_name = "PAIR")]
    pub(crate) pair: String,
    #[arg(long, value_name = "STEM")]
    pub(crate) output: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    Inspect,
    /// Validate every compact runtime metadata shard on the active installation.
    #[command(name = "metadata-qualification")]
    MetadataQualification(CatalogMetadataQualificationArgs),
    RomAudit(CatalogRomAuditArgs),
    #[command(name = "neogeo-family-audit")]
    NeoGeoFamilyAudit(CatalogNeoGeoFamilyAuditArgs),
    /// Export exact screenshot identities from one live Catalog V3 system shard.
    Screenshots(CatalogScreenshotsArgs),
    /// Qualify every published screenshot pack against the live catalog.
    #[command(name = "screenshot-qualification")]
    ScreenshotQualification(CatalogScreenshotQualificationArgs),
    Query(CatalogQueryArgs),
    Cores,
    /// Delete the Dev catalog and screenshot packs, then perform one supervised reboot.
    Purge(CatalogPurgeArgs),
}

#[derive(Debug, Args)]
pub struct CatalogMetadataQualificationArgs {
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Args)]
pub struct CatalogRomAuditArgs {
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Args)]
pub struct CatalogNeoGeoFamilyAuditArgs {
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Args)]
pub struct CatalogScreenshotsArgs {
    #[arg(long)]
    pub(crate) system: String,
    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Args)]
pub struct CatalogScreenshotQualificationArgs {
    /// Directory for per-system TSV, summary TSV, and summary JSON evidence.
    #[arg(long, value_name = "PATH")]
    pub(crate) out_dir: PathBuf,
    /// HTTPS manifest to qualify; defaults to the official unsigned v1 manifest.
    #[arg(long)]
    pub(crate) manifest_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct CatalogQueryArgs {
    #[arg(long)]
    pub(crate) database: String,
    #[arg(long)]
    pub(crate) sql: String,
}

#[derive(Debug, Args)]
pub struct CatalogPurgeArgs {
    #[arg(long, required = true)]
    attended: bool,
    #[arg(long, required = true)]
    reboot: bool,
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

#[derive(Debug, Subcommand)]
pub enum DeviceFpgaCommand {
    InstallExperimental(ExperimentalFpgaArgs),
    InstallExperimentalAgent(ExperimentalAgentArgs),
}

#[derive(Debug, Args)]
pub struct ExperimentalFpgaArgs {
    #[arg(long)]
    pub(crate) rbf: PathBuf,
    #[arg(long)]
    pub(crate) metadata: PathBuf,
    #[arg(long)]
    pub(crate) signoff_report: PathBuf,
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Args)]
pub struct ExperimentalAgentArgs {
    #[arg(long)]
    pub(crate) agent: PathBuf,
    #[arg(long)]
    pub(crate) expected_rbf_sha256: String,
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
            Self::TransferCheck(_) | Self::Scene(_) | Self::Reboot(_) => true,
            Self::Display { command } => !matches!(command, DisplayCommand::RouteStatus),
            Self::Crt { .. } => true,
            Self::Launcher { command } => !matches!(command, LauncherCommand::Status),
            Self::Catalog { command } => matches!(command, CatalogCommand::Purge(_)),
            Self::Media { command } => matches!(command, MediaCommand::Download(_)),
            Self::Fpga { .. } => true,
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

impl CrtFontExperiment {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PhaseEven => "phase-even",
            Self::CoverageMax => "coverage-max",
            Self::DominantRow => "dominant-row",
            Self::Xerxes => "xerxes",
            Self::XerxesPerfect => "xerxes-perfect",
            Self::YesterdayPerfect => "yesterday-perfect",
            Self::Bacteria => "bacteria",
            Self::BacteriaHalf => "bacteria-half",
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
        assert!(TestCli::try_parse_from(["test", "catalog", "purge"]).is_err());
        assert!(TestCli::try_parse_from(["test", "catalog", "purge", "--attended"]).is_err());
        assert!(TestCli::try_parse_from(["test", "catalog", "purge", "--reboot"]).is_err());
        assert!(
            TestCli::try_parse_from([
                "test",
                "fpga",
                "install-experimental",
                "--rbf",
                "candidate.rbf",
                "--metadata",
                "candidate.txt",
                "--signoff-report",
                "signoff.tsv",
            ])
            .is_err()
        );
        assert!(TestCli::try_parse_from(["test", "mode", "set", "dev", "--attended"]).is_ok());
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "launch-return-once",
                "--attended",
                "--output",
                "/tmp/launch-return-once",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "phase-even",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "fpga",
                "install-experimental-agent",
                "--agent",
                "mister-magik-agent",
                "--expected-rbf-sha256",
                "3701ec7e5ef7be168bc221fe208f41e8035e60d31d308ed3ecafcbb9a96ffde0",
                "--attended",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt240-composition",
                "legacy-480",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "yesterday-perfect",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "xerxes-perfect",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "bacteria-half",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "bacteria",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "xerxes",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "dominant-row",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "launcher",
                "restart",
                "--attended",
                "--crt-font-experiment",
                "coverage-max",
            ])
            .is_ok()
        );
        assert!(
            TestCli::try_parse_from(["test", "catalog", "purge", "--attended", "--reboot",])
                .is_ok()
        );
        assert!(
            TestCli::try_parse_from([
                "test",
                "fpga",
                "install-experimental",
                "--rbf",
                "candidate.rbf",
                "--metadata",
                "candidate.txt",
                "--signoff-report",
                "signoff.tsv",
                "--attended",
            ])
            .is_ok()
        );
    }

    #[test]
    fn retired_catalog_experiments_are_not_parseable() {
        for command in [
            "fast-five-prototype",
            "fast-five-c64-experiments",
            "fast-five-experiments",
            "fast-five-pprof",
            "fast-refresh-pprof",
            "fast-refresh-benchmark",
            "fast-source-ab",
            "fast-media-ab",
            "fast-five-old-cold",
        ] {
            let error = TestCli::try_parse_from(["test", "catalog", command])
                .err()
                .expect("retired command");
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::InvalidSubcommand,
                "{command}"
            );
        }
    }

    #[test]
    fn retained_catalog_commands_still_parse() {
        for args in [
            vec!["inspect"],
            vec!["cores"],
            vec!["metadata-qualification", "--out", "report.json"],
            vec!["rom-audit", "--out", "report.json"],
            vec!["neogeo-family-audit", "--out", "report.json"],
            vec!["screenshots", "--system", "arcade", "--out", "report.tsv"],
            vec!["screenshot-qualification", "--out-dir", "reports"],
            vec!["query", "--database", "catalog", "--sql", "SELECT 1"],
            vec!["purge", "--attended", "--reboot"],
        ] {
            assert!(TestCli::try_parse_from(["test", "catalog"].into_iter().chain(args)).is_ok());
        }
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
        let route_status = TestCli::try_parse_from(["test", "display", "route-status"]).unwrap();
        assert!(!route_status.command.is_mutation());
        let reboot = TestCli::try_parse_from(["test", "reboot", "--attended"]).unwrap();
        assert!(reboot.command.is_mutation());
        let purge = TestCli::try_parse_from(["test", "catalog", "purge", "--attended", "--reboot"])
            .unwrap();
        assert!(purge.command.is_mutation());
    }
}
