// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::architecture::ArchitectureCommand;
use crate::build::BuildCommand;
use crate::commands::device::DeviceCommand;
use crate::compile_time::CompileTimeCommand;
use crate::dependencies::DependenciesCommand;
use crate::fpga::FpgaCommand;
use crate::live_particles::LiveParticlesCommand;
use crate::model::{ArcadeVelocityScrollArm, ArcadeVelocityScrollRoute, BenchmarkScenario, Scope};
use crate::startup_particles::{SceneLabCommand, StartupParticlesCommand};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Human,
}

#[derive(Debug, Parser)]
#[command(
    name = "agent-cli",
    version,
    about = "MiSTer MagiK workflow harness",
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(skip = OutputFormat::Human)]
    pub output_format: OutputFormat,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // Parsed once; keeping Clap's command tree direct avoids dispatch indirection.
pub enum Command {
    #[command(hide = true)]
    PrePush {
        #[arg(long)]
        remote: String,
    },
    Plan(ScopeArgs),
    Architecture {
        #[command(subcommand)]
        command: ArchitectureCommand,
    },
    #[command(hide = true)]
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    #[command(hide = true)]
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    Diagnose,
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    Deliver {
        #[arg(value_enum)]
        target: Option<DeliverTarget>,
    },
    Benchmark {
        #[arg(value_enum, default_value_t)]
        scenario: BenchmarkScenario,
        #[arg(value_enum)]
        arm: Option<ArcadeVelocityScrollArm>,
        #[arg(long, value_enum, default_value_t)]
        route: ArcadeVelocityScrollRoute,
        #[arg(
            long,
            default_value_t = 40,
            value_parser = clap::value_parser!(u64).range(5..=120)
        )]
        duration_seconds: u64,
    },
    Capture {
        #[command(subcommand)]
        command: CaptureCommand,
    },
    Alpha {
        #[command(subcommand)]
        command: AlphaCommand,
    },
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    CompileTime {
        #[command(subcommand)]
        command: CompileTimeCommand,
    },
    LiveParticles {
        #[command(subcommand)]
        command: LiveParticlesCommand,
    },
    StartupParticles {
        #[command(subcommand)]
        command: StartupParticlesCommand,
    },
    SceneLab {
        #[command(subcommand)]
        command: SceneLabCommand,
    },
    /// Remove Cargo build artifacts from every project in the repository.
    Clean,
    Dependencies {
        #[command(subcommand)]
        command: DependenciesCommand,
    },
    /// Build and verify the matched FPGA signoff set locally on Apple Silicon.
    Fpga {
        #[command(subcommand)]
        command: FpgaCommand,
    },
    #[command(hide = true)]
    Build {
        #[arg(value_enum)]
        intent: BuildCommand,
    },
    #[command(hide = true)]
    Ci {
        #[command(subcommand)]
        command: CiCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CiCommand {
    HostAssurance(ScopeArgs),
    PlatformCandidates {
        artifacts: PathBuf,
        name: String,
    },
    PlatformEligibleRun {
        run: PathBuf,
        head_sha: String,
    },
    RequireAlphaPromotion {
        channel: String,
        alpha_sha: String,
        candidate_sha: String,
    },
    PlatformManifest {
        #[command(subcommand)]
        command: PlatformManifestCommand,
    },
    GameDatabases {
        #[command(subcommand)]
        command: GameDatabaseCommand,
    },
    PlatformBundle {
        #[command(subcommand)]
        command: PlatformBundleCommand,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DeliverTarget {
    LocalMain,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Subcommand)]
#[allow(clippy::large_enum_variant)] // Clap owns this short-lived value; boxing fields obscures its flat CI API.
pub enum PlatformBundleCommand {
    Create {
        #[arg(long)]
        main_dir: PathBuf,
        #[arg(long)]
        fpga_dir: PathBuf,
        #[arg(long)]
        scanout_dir: PathBuf,
        #[arg(long)]
        main_id: String,
        #[arg(long)]
        fpga_id: String,
        #[arg(long)]
        kernel_id: String,
        #[arg(long)]
        main_run_id: String,
        #[arg(long)]
        fpga_run_id: String,
        #[arg(long)]
        kernel_run_id: String,
        #[arg(long)]
        main_head_sha: String,
        #[arg(long)]
        fpga_head_sha: String,
        #[arg(long)]
        kernel_head_sha: String,
        #[arg(long)]
        main_source: String,
        #[arg(long)]
        fpga_source: String,
        #[arg(long)]
        kernel_source: String,
        #[arg(long)]
        release_version: u64,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        archive: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        release_version: Option<u64>,
    },
    ExtractComponent {
        archive: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        component: String,
        #[arg(long)]
        component_id: String,
        #[arg(long)]
        output: PathBuf,
    },
    VerifyComponent {
        #[arg(long)]
        component: String,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        component_id: String,
        #[arg(long)]
        revision: Option<String>,
    },
    CompactComponent {
        #[arg(long)]
        component: String,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        component_id: String,
    },
    WriteComponentCache {
        #[arg(long)]
        component: String,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        component_id: String,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        head_sha: String,
    },
    PlanUpdate {
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        current_version: u64,
        #[arg(long)]
        main_id: String,
        #[arg(long)]
        fpga_id: String,
        #[arg(long)]
        kernel_id: String,
        #[arg(long)]
        github_output: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Subcommand)]
pub enum GameDatabaseCommand {
    BuildMame {
        #[arg(long)]
        out: PathBuf,
        #[arg(long, conflicts_with_all = ["mame", "machine_sqlite"])]
        listxml: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["listxml", "machine_sqlite"])]
        mame: Option<PathBuf>,
        #[arg(long, conflicts_with_all = ["listxml", "mame"])]
        machine_sqlite: Option<PathBuf>,
        #[arg(long)]
        software_dir: Option<PathBuf>,
    },
    ImportArcade {
        #[arg(long)]
        sqlite: PathBuf,
        #[arg(long)]
        csv: PathBuf,
        #[arg(long)]
        source_sha: String,
    },
    Create {
        #[arg(long)]
        mame_sqlite: PathBuf,
        #[arg(long)]
        hbmame_sqlite: PathBuf,
        #[arg(long)]
        release_version: u64,
        #[arg(long)]
        mame_tag: String,
        #[arg(long)]
        mame_sha: String,
        #[arg(long)]
        mame_listxml_asset: String,
        #[arg(long)]
        mame_listxml_sha256: String,
        #[arg(long)]
        hbmame_tag: String,
        #[arg(long)]
        hbmame_sha: String,
        #[arg(long)]
        mame_builder_sha: String,
        #[arg(long)]
        hbmame_builder_sha: String,
        #[arg(long)]
        arcade_database_csv: PathBuf,
        #[arg(long)]
        arcade_database_license: PathBuf,
        #[arg(long)]
        arcade_database_sha: String,
        #[arg(long)]
        arcade_database_builder_sha: String,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        archive: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        checksums: Option<PathBuf>,
    },
    ExtractRelease {
        release: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    PlanUpdate {
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long)]
        mame_tag: String,
        #[arg(long)]
        mame_sha: String,
        #[arg(long)]
        hbmame_tag: String,
        #[arg(long)]
        hbmame_sha: String,
        #[arg(long)]
        arcade_database_sha: String,
        #[arg(long)]
        github_output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // CI-only parsed data; boxing a single field would obscure the schema.
pub enum PlatformManifestCommand {
    Generate {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        main: PathBuf,
        #[arg(long)]
        gui: PathBuf,
        #[arg(long)]
        manager: PathBuf,
        #[arg(long)]
        scanout_module: PathBuf,
        #[arg(long)]
        scanout_metadata: PathBuf,
        #[arg(long)]
        latch_rbf: PathBuf,
        #[arg(long)]
        latch_metadata: PathBuf,
        #[arg(long)]
        platform_bundle_manifest: PathBuf,
        #[arg(long)]
        main_revision: String,
        #[arg(long)]
        magik_revision: String,
        #[arg(long, default_value = "public")]
        layout: String,
    },
    Verify {
        manifest: PathBuf,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_value = "public")]
        layout: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    Qualify,
    FrameEvidence {
        #[command(subcommand)]
        command: FrameEvidenceCommand,
    },
    ReturnQualification {
        #[command(subcommand)]
        command: ReturnQualificationCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum FrameEvidenceCommand {
    Verify { evidence: PathBuf },
}

#[derive(Debug, Subcommand)]
pub enum ReturnQualificationCommand {
    RecordBoard {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long, default_value = "public")]
        layout: String,
        #[arg(long, value_name = "PATH", num_args = 1.., required = true)]
        frame_evidence: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, action = clap::ArgAction::SetTrue, required = true)]
        attended: bool,
    },
    Aggregate {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long, default_value = "public")]
        layout: String,
        #[arg(long, value_name = "PATH", num_args = 1.., required = true)]
        board_evidence: Vec<PathBuf>,
        #[arg(long, default_value = crate::return_qualification::DEFAULT_AGGREGATE_CERTIFICATE)]
        output: PathBuf,
    },
    VerifyAggregate {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long, default_value = "public")]
        layout: String,
        #[arg(long, default_value = crate::return_qualification::DEFAULT_AGGREGATE_CERTIFICATE)]
        certificate: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum AlphaCommand {
    Accept {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Reuse the identity-verified public alpha without reinstalling or rebooting.
        #[arg(long)]
        reuse_installed: bool,
        /// Restore the pre-test Main selection. By default the MiSTer stays on alpha.
        #[arg(long)]
        restore_host_mode: bool,
        /// Skip physical USB Video captures and retain authoritative framebuffer evidence only.
        #[arg(long)]
        framebuffer_only: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    UsbVideo {
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[arg(long, value_name = "SECONDS")]
        seconds: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    Show { run_id: String },
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    Report,
}

#[derive(Clone, Debug, Args)]
pub struct ScopeArgs {
    #[arg(long, value_name = "PATH", num_args = 1..)]
    pub paths: Vec<PathBuf>,
}

impl ScopeArgs {
    pub fn scope(&self) -> Scope {
        if !self.paths.is_empty() {
            Scope::Paths(self.paths.clone())
        } else {
            Scope::WorkingTree
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn bare_invocation_displays_help_instead_of_creating_an_intent() {
        assert!(Cli::try_parse_from(["agent-cli"]).is_err());
    }

    #[test]
    fn return_qualification_commands_are_closed_and_typed() {
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "frame-evidence",
                "verify",
                "/tmp/frame.json",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "return-qualification",
                "record-board",
                "--candidate",
                "/tmp/platform-v3.manifest",
                "--frame-evidence",
                "/tmp/frame.json",
                "--output",
                "/tmp/board.json",
                "--attended",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "return-qualification",
                "record-board",
                "--candidate",
                "/tmp/platform-v3.manifest",
                "--frame-evidence",
                "/tmp/frame.json",
                "--output",
                "/tmp/board.json",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "return-qualification",
                "aggregate",
                "--candidate",
                "/tmp/platform-v3.manifest",
                "--board-evidence",
                "/tmp/board-1.json",
                "/tmp/board-2.json",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "return-qualification",
                "aggregate",
                "--candidate",
                "/tmp/platform-v3.manifest",
            ])
            .is_err()
        );
    }

    #[test]
    fn card_flip_scene_lab_commands_are_typed_and_recipe_free() {
        assert!(
            Cli::try_parse_from(["agent-cli", "scene-lab", "preview", "--scene", "card-flip"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "scene-lab",
                "capture",
                "--scene",
                "card-flip",
                "--time-ms",
                "0",
                "--output",
                "/tmp/card-flip-front.ppm",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "device",
                "scene-lab",
                "--scene",
                "card-flip",
                "--assess",
                "--attended",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "device",
                "scene-lab",
                "--scene",
                "card-flip",
                "--profile",
                "--attended",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "scene-lab",
                "capture",
                "--scene",
                "card-flip",
                "--direction",
                "reverse",
                "--time-ms",
                "220",
                "--output",
                "/tmp/card-flip.ppm",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "device",
                "scene-lab",
                "--scene",
                "card-flip",
                "--attended",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "scene-lab",
                "capture",
                "--scene",
                "card-flip",
                "--direction",
                "sideways",
                "--time-ms",
                "220",
                "--output",
                "/tmp/card-flip.ppm",
            ])
            .is_err()
        );
    }

    #[test]
    fn compile_time_commands_require_closed_targets_and_explicit_paths() {
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "build",
                "framebuffer-lab-macos",
                "--target-dir",
                "/tmp/mac-target",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "measure",
                "framebuffer-lab-arm",
                "--target-dir",
                "/tmp/arm-target",
                "--output",
                "/tmp/arm.json",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "build",
                "magik-full-app-macos",
                "--target-dir",
                "/tmp/magik-mac-target",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "measure",
                "magik-full-app-arm",
                "--target-dir",
                "/tmp/magik-arm-target",
                "--output",
                "/tmp/magik-arm.json",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "build",
                "unknown",
                "--target-dir",
                "/tmp/target",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "measure",
                "framebuffer-lab-arm",
                "--target-dir",
                "/tmp/arm-target",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "compare-revisions",
                "--baseline-repository",
                "/tmp/baseline",
                "--candidate-repository",
                "/tmp/candidate",
                "--work-root",
                "/tmp/compile-comparison",
                "--output",
                "/tmp/compile-comparison.json",
                "--scenario",
                "pre-push-catalog",
                "--scenario",
                "arm-runtime-ci",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "compare-revisions",
                "--baseline-repository",
                "/tmp/baseline",
                "--candidate-repository",
                "/tmp/candidate",
                "--work-root",
                "/tmp/compile-comparison",
                "--output",
                "/tmp/compile-comparison.json",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "campaign",
                "magik-full-app-arm",
                "--target-dir",
                "/tmp/campaign-target",
                "--candidate-output",
                "/tmp/candidate.json",
                "--output",
                "/tmp/campaign.json",
                "--next-baseline",
                "/tmp/baseline-next.json",
            ])
            .is_ok()
        );
    }

    #[test]
    fn retired_validation_commands_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "check"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "verify"]).is_err());
    }

    #[test]
    fn clean_is_a_flag_free_repository_command() {
        assert!(Cli::try_parse_from(["agent-cli", "clean"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "clean", "--package", "catalog"]).is_err());
    }

    #[test]
    fn hidden_ci_assurance_preserves_explicit_paths() {
        let cli = Cli::try_parse_from(["agent-cli", "ci", "host-assurance", "--paths", "a", "b"])
            .unwrap();
        let Some(Command::Ci { command }) = cli.command else {
            panic!("expected ci command");
        };
        let CiCommand::HostAssurance(scope) = command else {
            panic!("expected host assurance");
        };
        assert_eq!(
            scope.scope(),
            Scope::Paths(vec![PathBuf::from("a"), PathBuf::from("b")])
        );
    }

    #[test]
    fn explicit_paths_are_preserved() {
        let cli = Cli::try_parse_from(["agent-cli", "plan", "--paths", "a", "b"]).unwrap();
        let Some(Command::Plan(scope)) = cli.command else {
            panic!("expected plan command");
        };
        assert_eq!(
            scope.scope(),
            Scope::Paths(vec![PathBuf::from("a"), PathBuf::from("b")])
        );
    }

    #[test]
    fn deliver_is_flag_free_and_git_independent() {
        assert!(Cli::try_parse_from(["agent-cli", "deliver"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "local-main"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--local-main"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--fast"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy-recipe", "launcher-device"]).is_err());
    }

    #[test]
    fn fpga_local_workflow_has_typed_setup_and_signoff_commands() {
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "setup"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "signoff"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "signoff", "--rebuild"]).is_ok());
    }

    #[test]
    fn benchmark_defaults_to_screensaver_and_accepts_typed_scenarios() {
        let accepted = [
            "screensaver",
            "catalog-lifecycle",
            "catalog-build-rebuild",
            "system-entry",
            "system-entry-critical",
            "system-entry-critical-confirm",
            "system-entry-critical-profile",
            "system-entry-critical-streamline",
            "system-entry-qualification",
            "particles",
            "particle-profile",
            "particle-capacity",
            "particle-demo-40k",
            "particle-step",
            "navigation-transitions",
            "settings-navigation",
            "settings-navigation-pprof",
            "orientation-transition-fade",
            "orientation-transition-zoom",
            "orientation-transition-fade-pprof",
            "orientation-transition-zoom-pprof",
            "pmu-profile",
            "launch-return",
            "launch-return-once",
            "launch-return-fallback",
            "launch-return-attribution",
            "modal-input",
            "input-integrity",
            "launcher-response",
            "launcher-response-attribution",
            "gui-frame-attribution",
            "scheduler-trace",
            "storage-attribution",
            "arcade-velocity-scroll",
            "arcade-velocity-scroll-attribution",
            "transition-streamline",
            "agent-observer-attribution",
            "agent-io-attribution",
            "input-latency-lab",
            "launcher-response-streamline",
            "cold-boot",
            "cold-boot-pprof",
            "search",
            "streamline",
        ];
        assert!(Cli::try_parse_from(["agent-cli", "benchmark"]).is_ok());
        for scenario in accepted {
            assert!(Cli::try_parse_from(["agent-cli", "benchmark", scenario]).is_ok());
        }
        for arm in ["control", "turbo", "pprof", "pmu", "streamline"] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "benchmark",
                    "arcade-velocity-scroll-attribution",
                    arm,
                ])
                .is_ok()
            );
        }
        for route in [
            "active",
            "hdmi-landscape",
            "hdmi-portrait-left",
            "hdmi-portrait-right",
            "hdmi1080-landscape",
            "hdmi1080-portrait-left",
            "crt240-portrait-left",
            "crt240-portrait-right",
            "crt288-portrait-left",
            "crt288-portrait-right",
        ] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "benchmark",
                    "arcade-velocity-scroll-attribution",
                    "control",
                    "--route",
                    route,
                ])
                .is_ok()
            );
        }
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "benchmark",
                "arcade-velocity-scroll-attribution",
                "turbo",
                "--route",
                "hdmi-portrait-left",
                "--duration-seconds",
                "20",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "benchmark",
                "arcade-velocity-scroll-attribution",
                "turbo",
                "--duration-seconds",
                "4",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "benchmark",
                "arcade-velocity-scroll-attribution",
                "control",
                "--route",
                "unknown",
            ])
            .is_err()
        );
        for removed in ["control-smoke", "pmu-smoke"] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "benchmark",
                    "arcade-velocity-scroll-attribution",
                    removed,
                ])
                .is_err()
            );
        }
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "benchmark",
                "arcade-velocity-scroll-attribution",
                "unknown",
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-01"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "firework-visual"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "--duration", "10"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "unknown"]).is_err());
    }

    #[test]
    fn removed_task_and_commit_surfaces_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "task", "begin"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "commit", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "check"]).is_err());
    }

    #[test]
    fn usb_video_capture_preserves_optional_output() {
        let cli = Cli::try_parse_from([
            "agent-cli",
            "capture",
            "usb-video",
            "--seconds",
            "24",
            "--output",
            "/tmp/probe.mov",
        ])
        .unwrap();
        let Some(Command::Capture {
            command: CaptureCommand::UsbVideo { output, seconds },
        }) = cli.command
        else {
            panic!("expected usb-video capture");
        };
        assert_eq!(output, Some(PathBuf::from("/tmp/probe.mov")));
        assert_eq!(seconds, Some(24));
    }

    #[test]
    fn release_qualify_is_flag_free() {
        assert!(Cli::try_parse_from(["agent-cli", "release", "qualify"]).is_ok());
        assert!(
            Cli::try_parse_from(["agent-cli", "release", "qualify", "--skip-display"]).is_err()
        );
    }

    #[test]
    fn alpha_accept_has_a_closed_candidate_and_output_interface() {
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "alpha",
                "accept",
                "--candidate",
                "/tmp/candidate",
                "--output",
                "/tmp/evidence",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "alpha",
                "accept",
                "--candidate",
                "/tmp/candidate",
                "--output",
                "/tmp/evidence",
                "--reuse-installed",
                "--restore-host-mode",
                "--framebuffer-only",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["agent-cli", "alpha", "accept"]).is_err());
    }

    #[test]
    fn diagnose_is_flag_free() {
        assert!(Cli::try_parse_from(["agent-cli", "diagnose"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "diagnose", "--repair-all"]).is_err());
    }

    #[test]
    fn git_hook_commands_have_closed_interfaces() {
        assert!(Cli::try_parse_from(["agent-cli", "pre-push", "--remote", "origin"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "pre-push"]).is_err());
    }

    #[test]
    fn build_accepts_only_owned_intents() {
        for intent in [
            "runtime-device",
            "runtime-ci",
            "runtime-analysis",
            "validate-launcher",
            "validate-library",
            "validate-runtime",
            "device-agent",
            "device-agent-ci",
            "manager-device",
            "framebuffer-scene-lab-device",
            "framebuffer-scene-lab-analysis",
            "release-binaries",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_ok());
        }
        for intent in [
            "runtime-profile",
            "host-tool",
            "runtime-benchmark",
            "runtime-diagnostics",
            "runtime-experiments",
            "catalog-builder",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_err());
        }
    }

    #[test]
    fn display_mode_operator_surface_is_not_available() {
        assert!(Cli::try_parse_from(["agent-cli", "display-mode", "8"]).is_err());
    }

    #[test]
    fn platform_bundle_output_paths_parse_without_a_global_output_mode() {
        let main_id = "a".repeat(64);
        let fpga_id = "b".repeat(64);
        let kernel_id = "c".repeat(64);
        let main_sha = "d".repeat(40);
        let fpga_sha = "e".repeat(40);
        let kernel_sha = "f".repeat(40);
        let create = [
            "agent-cli",
            "ci",
            "platform-bundle",
            "create",
            "--main-dir",
            "main",
            "--fpga-dir",
            "fpga",
            "--scanout-dir",
            "scanout",
            "--main-id",
            &main_id,
            "--fpga-id",
            &fpga_id,
            "--kernel-id",
            &kernel_id,
            "--main-run-id",
            "1",
            "--fpga-run-id",
            "2",
            "--kernel-run-id",
            "3",
            "--main-head-sha",
            &main_sha,
            "--fpga-head-sha",
            &fpga_sha,
            "--kernel-head-sha",
            &kernel_sha,
            "--main-source",
            "reused-from-actions-cache",
            "--fpga-source",
            "reused-from-actions-cache",
            "--kernel-source",
            "reused-from-actions-cache",
            "--release-version",
            "8",
            "--output",
            "bundle",
        ];
        let cli = Cli::try_parse_from(create.iter().copied()).unwrap();
        assert_eq!(cli.output_format, OutputFormat::Human);
        assert!(Cli::try_parse_from(["agent-cli", "--output-format", "ndjson", "plan"]).is_err());
        assert!(
            Cli::try_parse_from(
                create
                    .iter()
                    .copied()
                    .chain(["--main-workflow", "ignored.yml"]),
            )
            .is_err()
        );

        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "ci",
                "platform-bundle",
                "extract-component",
                "bundle.zip",
                "--manifest",
                "manifest.json",
                "--component",
                "kernel",
                "--component-id",
                &kernel_id,
                "--output",
                "kernel",
            ])
            .is_ok()
        );

        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "ci",
                "platform-bundle",
                "compact-component",
                "--component",
                "fpga",
                "--artifact",
                "legacy-fpga",
                "--component-id",
                &fpga_id,
                "--output",
                "compact-fpga",
            ])
            .is_ok()
        );
    }

    #[test]
    fn platform_manifest_output_path_parses() {
        let main_revision = "a".repeat(40);
        let magik_revision = "b".repeat(40);
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "ci",
                "platform-manifest",
                "generate",
                "--output",
                "platform.json",
                "--main",
                "MiSTer_MagiK",
                "--gui",
                "mister-magik-fb",
                "--manager",
                "mister-magik-manager",
                "--scanout-module",
                "scanout.ko",
                "--scanout-metadata",
                "scanout.txt",
                "--latch-rbf",
                "latch.rbf",
                "--latch-metadata",
                "latch.txt",
                "--platform-bundle-manifest",
                "platform-bundle-v0.2.json",
                "--main-revision",
                &main_revision,
                "--magik-revision",
                &magik_revision,
            ])
            .is_ok()
        );
    }

    #[test]
    fn architecture_report_requires_explicit_trees_and_accepts_both_formats() {
        for format in ["json", "markdown"] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "architecture",
                    "report",
                    "--base",
                    "base",
                    "--head",
                    "head",
                    "--format",
                    format,
                ])
                .is_ok()
            );
        }
        assert!(
            Cli::try_parse_from(["agent-cli", "architecture", "report", "--head", "head"]).is_err()
        );
    }
}
