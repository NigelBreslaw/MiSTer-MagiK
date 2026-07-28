// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::BuildCommand;
use crate::model::{BenchmarkScenario, Intent, Scope};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Ndjson,
}

#[derive(Debug, Parser)]
#[command(
    name = "agent-cli",
    version,
    about = "MiSTer MagiK workflow harness",
    arg_required_else_help = true
)]
pub struct Cli {
    #[arg(
        long = "output-format",
        value_enum,
        default_value_t = OutputFormat::Human,
        global = true
    )]
    pub output_format: OutputFormat,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    PrePush {
        #[arg(long)]
        remote: String,
    },
    Plan(ScopeArgs),
    #[command(hide = true)]
    Runs {
        #[arg(long)]
        failed: bool,
        #[arg(long, default_value_t = 20)]
        recent: usize,
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
    #[command(hide = true)]
    Doctor,
    Diagnose,
    Deliver,
    Benchmark {
        #[arg(value_enum, default_value_t)]
        scenario: BenchmarkScenario,
    },
    Demo {
        #[arg(value_enum)]
        demo: DemoCommand,
    },
    Capture {
        #[command(subcommand)]
        command: CaptureCommand,
    },
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    #[command(hide = true)]
    Build {
        #[arg(value_enum)]
        intent: BuildCommand,
    },
    #[command(hide = true)]
    Ci {
        #[command(subcommand)]
        command: Box<CiCommand>,
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
        #[arg(long)]
        main_workflow: Option<String>,
        #[arg(long)]
        fpga_workflow: Option<String>,
        #[arg(long)]
        kernel_workflow: Option<String>,
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
        github_output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DemoCommand {
    ParticleShowcase,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    Show { run_id: String },
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    Status,
    Report,
    Rotate,
    PruneLogs,
}

#[derive(Clone, Debug, Args)]
pub struct ScopeArgs {
    #[arg(long)]
    pub verbose: bool,
    #[arg(long, conflicts_with = "paths")]
    pub staged: bool,
    #[arg(long, value_name = "PATH", num_args = 1.., conflicts_with = "staged")]
    pub paths: Vec<PathBuf>,
}

impl ScopeArgs {
    fn into_scope(self) -> Scope {
        if self.staged {
            Scope::Staged
        } else if !self.paths.is_empty() {
            Scope::Paths(self.paths)
        } else {
            Scope::WorkingTree
        }
    }
}

impl Cli {
    #[must_use]
    pub fn into_intent(self) -> Intent {
        match self.command {
            None => unreachable!("clap requires a workflow command"),
            Some(Command::PrePush { remote }) => Intent::PrePush { remote },
            Some(Command::Plan(scope)) => Intent::Plan {
                verbose: scope.verbose,
                scope: scope.into_scope(),
            },
            Some(Command::Runs { failed, recent }) => Intent::ListRuns { failed, recent },
            Some(Command::Run {
                command: RunCommand::Show { run_id },
            }) => Intent::ShowRun { run_id },
            Some(Command::Db {
                command: DbCommand::Status,
            }) => Intent::DatabaseStatus,
            Some(Command::Db {
                command: DbCommand::Report,
            }) => Intent::DatabaseReport,
            Some(Command::Db {
                command: DbCommand::Rotate,
            }) => Intent::DatabaseRotate,
            Some(Command::Db {
                command: DbCommand::PruneLogs,
            }) => Intent::PruneLogs,
            Some(Command::Doctor) => Intent::Doctor,
            Some(Command::Diagnose) => Intent::Diagnose,
            Some(Command::Deliver) => Intent::Deliver,
            Some(Command::Benchmark { scenario }) => Intent::Benchmark { scenario },
            Some(Command::Demo {
                demo: DemoCommand::ParticleShowcase,
            }) => Intent::LaunchParticleShowcase,
            Some(Command::Capture {
                command: CaptureCommand::UsbVideo { output, seconds },
            }) => Intent::CaptureUsbVideo { output, seconds },
            Some(Command::Release {
                command: ReleaseCommand::Qualify,
            }) => Intent::ReleaseQualify,
            Some(Command::Build { intent }) => Intent::Build { intent },
            Some(Command::Ci { command }) => match *command {
                CiCommand::HostAssurance(scope) => Intent::CiHostAssurance {
                    scope: scope.into_scope(),
                },
                CiCommand::PlatformCandidates { artifacts, name } => {
                    Intent::CiPlatformCandidates { artifacts, name }
                }
                CiCommand::PlatformEligibleRun { run, head_sha } => {
                    Intent::CiPlatformEligibleRun { run, head_sha }
                }
                CiCommand::RequireAlphaPromotion {
                    channel,
                    alpha_sha,
                    candidate_sha,
                } => Intent::CiRequireAlphaPromotion {
                    channel,
                    alpha_sha,
                    candidate_sha,
                },
                CiCommand::PlatformManifest { command } => match command {
                    PlatformManifestCommand::Generate {
                        output,
                        main,
                        gui,
                        manager,
                        scanout_module,
                        scanout_metadata,
                        latch_rbf,
                        latch_metadata,
                        main_revision,
                        magik_revision,
                        layout,
                    } => Intent::CiPlatformManifestGenerate {
                        output,
                        main,
                        gui,
                        manager,
                        scanout_module,
                        scanout_metadata,
                        latch_rbf,
                        latch_metadata,
                        main_revision,
                        magik_revision,
                        layout,
                    },
                    PlatformManifestCommand::Verify {
                        manifest,
                        root,
                        layout,
                    } => Intent::CiPlatformManifestVerify {
                        manifest,
                        root,
                        layout,
                    },
                },
                CiCommand::GameDatabases { command } => Intent::CiGameDatabases { command },
                CiCommand::PlatformBundle { command } => Intent::CiPlatformBundle { command },
            },
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
    fn retired_validation_commands_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "check"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "verify"]).is_err());
    }

    #[test]
    fn hidden_ci_assurance_preserves_explicit_paths() {
        let cli = Cli::try_parse_from(["agent-cli", "ci", "host-assurance", "--paths", "a", "b"])
            .unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::CiHostAssurance {
                scope: Scope::Paths(vec![PathBuf::from("a"), PathBuf::from("b")])
            }
        );
    }

    #[test]
    fn explicit_paths_are_preserved() {
        let cli = Cli::try_parse_from(["agent-cli", "plan", "--paths", "a", "b"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Plan {
                scope: Scope::Paths(vec![PathBuf::from("a"), PathBuf::from("b")]),
                verbose: false,
            }
        );
    }

    #[test]
    fn deliver_is_flag_free_and_git_independent() {
        let cli = Cli::try_parse_from(["agent-cli", "deliver"]).unwrap();
        assert_eq!(cli.into_intent(), Intent::Deliver);
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--local-main"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--fast"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy-recipe", "launcher-device"]).is_err());
    }

    #[test]
    fn benchmark_defaults_to_screensaver_and_accepts_typed_scenarios() {
        let cli = Cli::try_parse_from(["agent-cli", "benchmark"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::Screensaver
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "catalog-lifecycle"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::CatalogLifecycle
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particles"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::Particles
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-profile"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleProfile
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-capacity"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleCapacity
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-40k"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleDemo40k
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-step"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleStep
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-01"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleDemo01
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-profile-01"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleDemoProfile01
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-10"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleDemo10
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "particle-demo-profile-10"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::ParticleDemoProfile10
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "benchmark", "search"])
                .unwrap()
                .into_intent(),
            Intent::Benchmark {
                scenario: BenchmarkScenario::Search
            }
        );
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
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "capture", "usb-video"])
                .unwrap()
                .into_intent(),
            Intent::CaptureUsbVideo {
                output: None,
                seconds: None,
            }
        );
        assert_eq!(
            Cli::try_parse_from([
                "agent-cli",
                "capture",
                "usb-video",
                "--output",
                "/tmp/frame.jpg",
            ])
            .unwrap()
            .into_intent(),
            Intent::CaptureUsbVideo {
                output: Some(PathBuf::from("/tmp/frame.jpg")),
                seconds: None,
            }
        );
        assert_eq!(
            Cli::try_parse_from([
                "agent-cli",
                "capture",
                "usb-video",
                "--seconds",
                "24",
                "--output",
                "/tmp/probe.mov",
            ])
            .unwrap()
            .into_intent(),
            Intent::CaptureUsbVideo {
                output: Some(PathBuf::from("/tmp/probe.mov")),
                seconds: Some(24),
            }
        );
    }

    #[test]
    fn release_qualify_is_flag_free() {
        let cli = Cli::try_parse_from(["agent-cli", "release", "qualify"]).unwrap();
        assert_eq!(cli.into_intent(), Intent::ReleaseQualify);
        assert!(
            Cli::try_parse_from(["agent-cli", "release", "qualify", "--skip-display"]).is_err()
        );
    }

    #[test]
    fn diagnose_is_flag_free() {
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "diagnose"])
                .unwrap()
                .into_intent(),
            Intent::Diagnose
        );
        assert!(Cli::try_parse_from(["agent-cli", "diagnose", "--repair-all"]).is_err());
    }

    #[test]
    fn git_hook_commands_have_closed_interfaces() {
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "pre-push", "--remote", "origin"])
                .unwrap()
                .into_intent(),
            Intent::PrePush {
                remote: "origin".into()
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "pre-push"]).is_err());
    }

    #[test]
    fn build_accepts_only_owned_intents() {
        for intent in [
            "runtime-device",
            "runtime-fast",
            "runtime-profile",
            "validate-launcher",
            "validate-library",
            "validate-runtime",
            "device-agent",
            "manager-device",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_ok());
        }
        for intent in [
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
    fn platform_bundle_output_paths_do_not_collide_with_output_format() {
        let main_id = "a".repeat(64);
        let fpga_id = "b".repeat(64);
        let kernel_id = "c".repeat(64);
        let main_sha = "d".repeat(40);
        let fpga_sha = "e".repeat(40);
        let kernel_sha = "f".repeat(40);
        let create = [
            "agent-cli",
            "--output-format",
            "ndjson",
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
        let cli = Cli::try_parse_from(create).unwrap();
        assert_eq!(cli.output_format, OutputFormat::Ndjson);

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
                "--main-revision",
                &main_revision,
                "--magik-revision",
                &magik_revision,
            ])
            .is_ok()
        );
    }
}
