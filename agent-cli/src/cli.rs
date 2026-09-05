// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::BuildCommand;
use crate::commands::device::DeviceCommand;
use crate::compile_time::CompileTimeCommand;
use crate::dependencies::DependenciesCommand;
use crate::fpga::FpgaCommand;
use crate::model::BenchmarkScenario;
use clap::{Parser, Subcommand, ValueEnum};
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

impl Cli {
    /// Parse the command tree and keep local database input on delivery lanes
    /// that support the isolated database transaction.
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = <Self as Parser>::try_parse_from(itr)?;
        if let Some(Command::Deliver {
            target,
            game_databases_release_dir,
        }) = &cli.command
        {
            match (target, game_databases_release_dir) {
                (DeliverTarget::GameDatabases, None) => {
                    return Err(clap::Error::raw(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "deliver game-databases requires --game-databases-release-dir PATH",
                    ));
                }
                (DeliverTarget::LocalMain, Some(_)) => {
                    return Err(clap::Error::raw(
                        clap::error::ErrorKind::ArgumentConflict,
                        "--game-databases-release-dir is not valid with deliver local-main",
                    ));
                }
                _ => {}
            }
        }
        Ok(cli)
    }
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // Parsed once; keeping Clap's command tree direct avoids dispatch indirection.
pub enum Command {
    /// Print the bounded guidance and authority record for one path.
    Guidance {
        path: PathBuf,
        #[arg(long)]
        json: bool,
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
    /// Deliver the retained platform or database transaction; app development uses scripts/magik2.
    Deliver {
        #[arg(value_enum)]
        target: DeliverTarget,
        #[arg(
            long,
            value_name = "PATH",
            help = "Use a locally verified game-database release directory"
        )]
        game_databases_release_dir: Option<PathBuf>,
    },
    /// Run an explicit legacy qualification workload; everyday measurements use scripts/magik2 check.
    Benchmark {
        #[arg(value_enum)]
        scenario: BenchmarkScenario,
    },
    Capture {
        #[command(subcommand)]
        command: CaptureCommand,
    },

    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    CompileTime {
        #[command(subcommand)]
        command: CompileTimeCommand,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DeliverTarget {
    Platform,
    LocalMain,
    GameDatabases,
}

/* CI artifact commands are implemented by scripts/magik_ci. */
/*
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
    BuildUpdaterArcade {
        #[arg(long)]
        input_manifest: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
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
        arcade_updater_builder_sha: String,
        #[arg(long)]
        arcade_updater_index: PathBuf,
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
        arcade_updater_builder_sha: String,
        #[arg(long = "arcade-updater-revision", value_name = "ID=SHA")]
        arcade_updater_revisions: Vec<String>,
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
*/

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_experiment_commands_are_not_available() {
        for command in ["live-particles", "startup-particles", "scene-lab"] {
            assert!(Cli::try_parse_from(["agent-cli", command, "preview"]).is_err());
            assert!(Cli::try_parse_from(["agent-cli", "device", command, "--attended"]).is_err());
        }
        for target in [
            "framebuffer-lab-device",
            "framebuffer-scene-lab-device",
            "framebuffer-scene-lab-analysis",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", target]).is_err());
        }
        for target in [
            "framebuffer-lab-arm",
            "framebuffer-lab-macos",
            "framebuffer-scene-lab-arm",
            "framebuffer-scene-lab-macos",
        ] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "compile-time",
                    "build",
                    target,
                    "--target-dir",
                    "/tmp/retired-target"
                ])
                .is_err()
            );
        }
        for edit in ["shared-navigation", "shared-screenshot-parade", "lab-host"] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "compile-time",
                    "measure",
                    "magik-full-app-macos",
                    "--edit",
                    edit,
                    "--target-dir",
                    "/tmp/retired-target",
                    "--output",
                    "/tmp/retired.json"
                ])
                .is_err()
            );
        }
        for scenario in [
            "particles",
            "particle-capacity",
            "particle-demo-40k",
            "particle-step",
            "particle-profile",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "benchmark", scenario]).is_err());
        }
    }

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
    fn compile_time_commands_require_closed_targets_and_explicit_paths() {
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "compile-time",
                "build",
                "magik-full-app-macos",
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
                "magik-full-app-arm",
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
                "magik-full-app-arm",
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
    fn guidance_accepts_exactly_one_path() {
        let cli = Cli::try_parse_from([
            "agent-cli",
            "guidance",
            "apps/mister/src/ui_runner/launcher_loop.rs",
        ])
        .unwrap();
        let Some(Command::Guidance { path, json }) = cli.command else {
            panic!("expected guidance command");
        };
        assert!(!json);
        assert!(matches!(
            Cli::try_parse_from(["agent-cli", "guidance", "--json", "a"])
                .unwrap()
                .command,
            Some(Command::Guidance { json: true, .. })
        ));
        assert_eq!(
            path,
            PathBuf::from("apps/mister/src/ui_runner/launcher_loop.rs")
        );
        assert!(Cli::try_parse_from(["agent-cli", "guidance"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "guidance", "a", "b"]).is_err());
    }

    #[test]
    fn deliver_accepts_only_the_bounded_local_database_input() {
        assert!(Cli::try_parse_from(["agent-cli", "deliver"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "local-main"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "deliver",
                "game-databases",
                "--game-databases-release-dir",
                "build/game-databases"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "game-databases"]).is_err());
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "deliver",
                "platform",
                "--game-databases-release-dir",
                "build/game-databases"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "deliver",
                "--game-databases-release-dir",
                "build/game-databases"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "deliver",
                "local-main",
                "--game-databases-release-dir",
                "build/game-databases"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--local-main"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--fast"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy-recipe", "launcher-device"]).is_err());
    }

    #[test]
    fn retired_app_workflows_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "restart-ui"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "runtime"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "platform"]).is_ok());
    }

    #[test]
    fn fpga_local_workflow_has_typed_setup_and_signoff_commands() {
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "setup"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "signoff"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "fpga", "signoff", "--rebuild"]).is_ok());
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
    fn diagnose_is_flag_free() {
        assert!(Cli::try_parse_from(["agent-cli", "diagnose"]).is_ok());
        assert!(Cli::try_parse_from(["agent-cli", "diagnose", "--repair-all"]).is_err());
    }

    #[test]
    fn retired_rust_assurance_commands_are_not_available() {
        assert!(Cli::try_parse_from(["agent-cli", "pre-push", "--remote", "origin"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "plan"]).is_err());
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
            "release-binaries",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_ok());
        }
        for intent in [
            "arcade-catalog-prototype-device",
            "five-system-catalog-prototype-device",
            "five-system-catalog-prototype-analysis",
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
    fn retired_application_qualification_is_unavailable() {
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "input-integrity"]).is_ok());
        for scenario in [
            "screensaver",
            "cold-boot",
            "catalog-lifecycle",
            "settings-navigation",
            "scheduler-trace",
            "arcade-catalog-prototype-cold",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "benchmark", scenario]).is_err());
        }
        assert!(Cli::try_parse_from(["agent-cli", "benchmark"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "alpha", "accept"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "device", "launcher", "ui-test"]).is_err());
        assert!(
            Cli::try_parse_from(["agent-cli", "device", "launcher", "ui-test-bridge"]).is_err()
        );
    }
}
