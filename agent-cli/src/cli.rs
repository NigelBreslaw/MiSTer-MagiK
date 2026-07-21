// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

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
    Runs {
        #[arg(long)]
        failed: bool,
        #[arg(long, default_value_t = 20)]
        recent: usize,
    },
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    Scripts {
        #[command(subcommand)]
        command: ScriptsCommand,
    },
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    Verify(ScopeArgs),
    Doctor,
    Deploy,
    #[command(hide = true)]
    DeployRecipe {
        #[arg(value_parser = [
            "launcher-device", "launcher-fast", "launcher-bench-device",
            "launcher-bench-fast", "launcher-diagnostics-device",
            "all-diagnostics-device", "launcher-profile", "all-scenes-profile",
            "all-experiments-device", "all-experiments-bench-device"
        ])]
        recipe: String,
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
pub enum ScriptsCommand {
    Review,
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
            Some(Command::Scripts {
                command: ScriptsCommand::Review,
            }) => Intent::ReviewScripts,
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
            Some(Command::Deploy) => Intent::Deploy { task_id },
            Some(Command::DeployRecipe { recipe }) => Intent::DeployRecipe { recipe },
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
    fn deploy_is_flag_free_and_task_scoped() {
        let cli = Cli::try_parse_from(["agent-cli", "--task-id", "task-1", "deploy"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Deploy {
                task_id: "task-1".into(),
            }
        );
        assert!(Cli::try_parse_from(["agent-cli", "deploy", "--fast"]).is_err());
        assert!(Cli::try_parse_from(["agent-cli", "deploy", "--ui-scope", "launcher"]).is_err());
    }
}
