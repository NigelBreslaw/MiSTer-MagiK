// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::commands::device::DeviceCommand;
use crate::model::BenchmarkScenario;
use clap::{Parser, Subcommand};
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
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Run an explicit legacy qualification workload; everyday measurements use scripts/magik2 check.
    Benchmark {
        #[arg(value_enum)]
        scenario: BenchmarkScenario,
    },

    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
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
    VerifyAggregate {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long, default_value = "public")]
        layout: String,
        #[arg(long, default_value = crate::return_qualification::DEFAULT_AGGREGATE_CERTIFICATE)]
        certificate: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphan_commands_are_rejected_and_neighbors_remain() {
        for args in [
            vec!["agent", "run", "show", "fixture"],
            vec![
                "agent",
                "device",
                "transfer-check",
                "--artifact",
                "fixture",
                "--attended",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        for args in [
            vec!["agent", "device", "arming-status"],
            vec!["agent", "device", "capture", "framebuffer"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }

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
    fn offline_readers_remain_and_certificate_generators_are_removed() {
        assert!(
            Cli::try_parse_from([
                "agent-cli",
                "release",
                "frame-evidence",
                "verify",
                "fixture.json"
            ])
            .is_ok()
        );
        for name in ["record-board", "aggregate"] {
            assert!(
                Cli::try_parse_from([
                    "agent-cli",
                    "release",
                    "return-qualification",
                    name,
                    "--candidate",
                    "fixture.manifest"
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn retired_compile_campaign_is_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "compile-time", "campaign"]).is_err());
    }

    #[test]
    fn retired_validation_commands_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "check"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "verify"]).is_err());
    }

    #[test]
    fn removed_host_maintenance_commands_are_rejected() {
        for args in [
            vec!["agent-cli", "clean"],
            vec!["agent-cli", "dependencies", "sync", "Cargo.toml"],
            vec!["agent-cli", "release", "qualify"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn retired_app_workflows_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "restart-ui"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "runtime"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "platform"]).is_err());
    }

    #[test]
    fn removed_task_and_commit_surfaces_are_rejected() {
        assert!(Cli::try_parse_from(["agent-cli", "task", "begin"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "commit", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "check"]).is_err());
    }

    #[test]
    fn retired_rust_assurance_commands_are_not_available() {
        assert!(Cli::try_parse_from(["agent-cli", "pre-push", "--remote", "origin"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "plan"]).is_err());
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
