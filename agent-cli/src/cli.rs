// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::BuildIntent;
use crate::model::{Intent, Scope};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Ndjson,
}

#[derive(Debug, Parser)]
#[command(name = "agent-cli", version, about = "MiSTer MagiK workflow harness")]
pub struct Cli {
    #[arg(long, value_enum, default_value_t = OutputFormat::Human, global = true)]
    pub output: OutputFormat,
    #[arg(long, global = true)]
    pub task_id: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Commit(CommitArgs),
    Plan(ScopeArgs),
    Check(ScopeArgs),
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
    Verify(ScopeArgs),
    #[command(hide = true)]
    Doctor,
    Diagnose,
    #[command(hide = true)]
    DisplayMode {
        #[arg(value_parser = ["6", "13", "14"])]
        video_mode: String,
        #[arg(long, value_parser = ["stock", "dev"])]
        main: Option<String>,
    },
    Deliver,
    Benchmark,
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    #[command(hide = true)]
    Build {
        #[arg(value_enum)]
        intent: BuildIntent,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    Begin {
        #[arg(long)]
        replace: bool,
    },
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    Qualify,
}

#[derive(Clone, Debug, Args)]
pub struct CommitArgs {
    #[arg(short = 'm', long, required = true)]
    pub message: String,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    Show { run_id: String },
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    Status,
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
    fn into_scope(self, task_id: Option<&str>) -> Scope {
        if self.staged {
            Scope::Staged
        } else if !self.paths.is_empty() {
            Scope::Paths(self.paths)
        } else {
            Scope::Task(task_id.unwrap_or("").to_owned())
        }
    }
}

impl Cli {
    #[must_use]
    pub fn into_intent(self) -> Intent {
        let task_id = self
            .task_id
            .or_else(|| std::env::var("MISTER_AGENT_TASK_ID").ok())
            .or_else(|| std::env::var("CODEX_THREAD_ID").ok())
            .unwrap_or_default();
        match self.command {
            None => Intent::Interactive,
            Some(Command::Task {
                command: TaskCommand::Begin { replace },
            }) => Intent::TaskBegin {
                task_id: if task_id.is_empty() {
                    generated_task_id()
                } else {
                    task_id
                },
                replace,
            },
            Some(Command::Task {
                command: TaskCommand::Status,
            }) => Intent::TaskStatus { task_id },
            Some(Command::Commit(args)) => Intent::Commit {
                task_id,
                message: args.message,
            },
            Some(Command::Plan(scope)) => Intent::Plan {
                verbose: scope.verbose,
                scope: scope.into_scope(Some(&task_id)),
            },
            Some(Command::Check(scope)) => Intent::Check {
                scope: scope.into_scope(Some(&task_id)),
            },
            Some(Command::Runs { failed, recent }) => Intent::ListRuns { failed, recent },
            Some(Command::Run {
                command: RunCommand::Show { run_id },
            }) => Intent::ShowRun { run_id },
            Some(Command::Db {
                command: DbCommand::Status,
            }) => Intent::DatabaseStatus,
            Some(Command::Db {
                command: DbCommand::PruneLogs,
            }) => Intent::PruneLogs,
            Some(Command::Verify(scope)) => Intent::Verify {
                scope: scope.into_scope(Some(&task_id)),
            },
            Some(Command::Doctor) => Intent::Doctor,
            Some(Command::Diagnose) => Intent::Diagnose,
            Some(Command::DisplayMode { video_mode, main }) => {
                Intent::DisplayMode { video_mode, main }
            }
            Some(Command::Deliver) => Intent::Deliver { task_id },
            Some(Command::Benchmark) => Intent::Benchmark { task_id },
            Some(Command::Release {
                command: ReleaseCommand::Qualify,
            }) => Intent::ReleaseQualify,
            Some(Command::Build { intent }) => Intent::Build { intent },
        }
    }
}

fn generated_task_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("task-{nanos:x}-{:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn check_defaults_to_task() {
        let cli = Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "check"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Check {
                scope: Scope::Task("task-1".into())
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
    fn commit_requires_and_preserves_message() {
        let cli = Cli::try_parse_from([
            "agent-cli",
            "--task-id",
            "task-1",
            "commit",
            "-m",
            "Update workflow",
        ])
        .unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Commit {
                task_id: "task-1".into(),
                message: "Update workflow".into(),
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "commit"]).is_err());
    }

    #[test]
    fn deliver_is_flag_free_and_task_scoped() {
        let cli = Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "deliver"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Deliver {
                task_id: "task-1".into(),
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "--fast"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deliver", "-m", "message"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy-recipe", "launcher-device"]).is_err());
    }

    #[test]
    fn benchmark_is_flag_free_and_task_scoped() {
        let cli = Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "benchmark"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Benchmark {
                task_id: "task-1".into()
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "benchmark", "--duration", "10"]).is_err());
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
    fn build_accepts_only_owned_intents() {
        for intent in [
            "runtime-device",
            "runtime-fast",
            "runtime-benchmark",
            "runtime-profile",
            "validate-launcher",
            "validate-library",
            "device-agent",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_ok());
        }
        for intent in [
            "host-tool",
            "runtime-diagnostics",
            "runtime-experiments",
            "catalog-builder",
        ] {
            assert!(Cli::try_parse_from(["agent-cli", "build", intent]).is_err());
        }
    }

    #[test]
    fn focused_display_mode_accepts_only_stress_presets() {
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "display-mode", "6"])
                .unwrap()
                .into_intent(),
            Intent::DisplayMode {
                video_mode: "6".into(),
                main: None,
            }
        );
        assert_eq!(
            Cli::try_parse_from(["agent-cli", "display-mode", "14", "--main", "stock"])
                .unwrap()
                .into_intent(),
            Intent::DisplayMode {
                video_mode: "14".into(),
                main: Some("stock".into()),
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "display-mode", "10"]).is_err());
    }
}
