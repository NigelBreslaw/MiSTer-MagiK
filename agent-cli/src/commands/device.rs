// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::AgentResult;
use clap::{Args, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

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
    LiveParticles(LiveParticlesArgs),
    StartupParticles(StartupParticlesArgs),
    SceneLab(SceneLabArgs),
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

#[derive(Debug, Args)]
pub struct LiveParticlesArgs {
    pub(crate) family: PathBuf,
    #[arg(long)]
    pub(crate) demo: String,
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Args)]
pub struct StartupParticlesArgs {
    pub(crate) recipe: PathBuf,
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Debug, Args)]
pub struct SceneLabArgs {
    #[arg(long, value_enum)]
    pub(crate) scene: SceneLabScene,
    #[arg(long)]
    pub(crate) recipe: Option<PathBuf>,
    #[arg(long)]
    pub(crate) fixture: Option<String>,
    #[arg(long)]
    pub(crate) seed: Option<String>,
    #[arg(long)]
    pub(crate) case: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=524_288))]
    pub(crate) particle_count: Option<u32>,
    #[arg(long, value_enum)]
    pub(crate) particle_preset: Option<SceneLabParticlePreset>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=600))]
    pub(crate) seconds: Option<u64>,
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=600))]
    pub(crate) warmup_seconds: u64,
    #[arg(long)]
    pub(crate) profile: bool,
    #[arg(long)]
    pub(crate) assess: bool,
    #[arg(long)]
    pub(crate) pmu: bool,
    #[arg(long, required = true)]
    attended: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SceneLabScene {
    Magik,
    Cabinet,
    Intro,
    NavigationTransition,
    CardFlip,
    ScreenshotScreensaver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SceneLabParticlePreset {
    Capacity,
    Visual,
}

impl SceneLabParticlePreset {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::Visual => "visual",
        }
    }
}

impl SceneLabScene {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Magik => "magik",
            Self::Cabinet => "cabinet",
            Self::Intro => "intro",
            Self::NavigationTransition => "navigation-transition",
            Self::CardFlip => "card-flip",
            Self::ScreenshotScreensaver => "screenshot-screensaver",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum LauncherCommand {
    Status,
    Restart(LauncherRestartArgs),
    CaptureFirstArcade(FirstArcadeCaptureArgs),
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
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CrtFontExperiment {
    PhaseEven,
    CoverageMax,
    DominantRow,
    Xerxes,
    Bacteria,
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
    Query(CatalogQueryArgs),
    Cores,
    /// Delete the Dev catalog and screenshot packs, then perform one supervised reboot.
    Purge(CatalogPurgeArgs),
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

pub fn run(command: DeviceCommand) -> AgentResult<()> {
    if command.requires_repository() {
        return Err("live particle sessions require the repository workflow".into());
    }
    let mutation = command.is_mutation();
    let mut device = crate::device::DeviceClient::default();
    if mutation {
        device.mutate(|device| device.run_operator(&command))
    } else {
        device.read(|device| device.run_operator(&command))
    }
}

pub fn run_live_particles(args: &LiveParticlesArgs, binary: &Path) -> AgentResult<()> {
    let mut device = crate::device::DeviceClient::default();
    device.mutate(|device| device.run_live_particles(binary, &args.family, &args.demo))
}

pub fn run_startup_particles(args: &StartupParticlesArgs, binary: &Path) -> AgentResult<()> {
    let mut device = crate::device::DeviceClient::default();
    device.mutate(|device| device.run_startup_particles(binary, &args.recipe))
}

pub fn run_scene_lab(
    args: &SceneLabArgs,
    binary: &Path,
    output_dir: Option<&Path>,
) -> AgentResult<()> {
    let seed = args
        .seed
        .as_deref()
        .map(crate::startup_particles::parse_screenshot_seed)
        .transpose()?;
    let mut device = crate::device::DeviceClient::default();
    device.mutate(|device| {
        device.run_scene_lab(crate::host::SceneLabRequest {
            binary,
            scene: args.scene.as_str(),
            recipe: args.recipe.as_deref(),
            fixture: args.fixture.as_deref(),
            seed,
            case: args.case.as_deref(),
            particle_count: args.particle_count,
            particle_preset: args.particle_preset.map(SceneLabParticlePreset::as_str),
            seconds: args.seconds,
            warmup_seconds: args.warmup_seconds,
            profile: args.profile,
            assess: args.assess,
            pmu: args.pmu,
            output_dir,
        })
    })
}

impl DeviceCommand {
    pub fn requires_repository(&self) -> bool {
        matches!(
            self,
            Self::LiveParticles(_) | Self::StartupParticles(_) | Self::SceneLab(_)
        )
    }

    pub(crate) fn is_mutation(&self) -> bool {
        match self {
            Self::Status(_)
            | Self::ArmingStatus
            | Self::Logs
            | Self::Events
            | Self::Diagnostics(_)
            | Self::Capture { .. } => false,
            Self::Mode { command } => matches!(command, ModeCommand::Set(_)),
            Self::Scene(_)
            | Self::Reboot(_)
            | Self::LiveParticles(_)
            | Self::StartupParticles(_)
            | Self::SceneLab(_) => true,
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
            Self::Bacteria => "bacteria",
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
