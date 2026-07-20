// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Intent, RustTask, Scope};
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
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Lint(ScopeArgs),
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
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
    Verify {
        #[command(subcommand)]
        command: VerifyCommand,
    },
    Doctor,
    Rust {
        #[command(subcommand)]
        task: RustCommand,
    },
    HostTools {
        #[arg(long)]
        full: bool,
    },
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum VerifyCommand {
    FullHost,
}

#[derive(Debug, Subcommand)]
pub enum RustCommand {
    Fmt,
    Test,
    Check,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    Host,
}

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    Lint(ScopeArgs),
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
#[group(required = false, multiple = false)]
pub struct ScopeArgs {
    #[arg(long)]
    pub staged: bool,
    #[arg(long)]
    pub working_tree: bool,
    #[arg(long, value_name = "PATH", num_args = 1..)]
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
            None => Intent::Interactive,
            Some(Command::Lint(scope)) => Intent::Lint {
                scope: scope.into_scope(),
            },
            Some(Command::Plan {
                command: PlanCommand::Lint(scope),
            }) => Intent::PlanLint {
                scope: scope.into_scope(),
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
            Some(Command::Verify {
                command: VerifyCommand::FullHost,
            }) => Intent::VerifyFullHost,
            Some(Command::Doctor) => Intent::Doctor,
            Some(Command::Rust { task }) => Intent::Rust {
                task: match task {
                    RustCommand::Fmt => RustTask::Format,
                    RustCommand::Test => RustTask::Test,
                    RustCommand::Check => RustTask::Check,
                },
            },
            Some(Command::HostTools { full }) => Intent::HostTools { full },
            Some(Command::Release {
                command: ReleaseCommand::Host,
            }) => Intent::ReleaseHost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn lint_defaults_to_working_tree() {
        let cli = Cli::try_parse_from(["agent-cli", "lint"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::Lint {
                scope: Scope::WorkingTree
            }
        );
    }

    #[test]
    fn explicit_paths_are_preserved() {
        let cli = Cli::try_parse_from(["agent-cli", "plan", "lint", "--paths", "a", "b"]).unwrap();
        assert_eq!(
            cli.into_intent(),
            Intent::PlanLint {
                scope: Scope::Paths(vec![PathBuf::from("a"), PathBuf::from("b")])
            }
        );
    }
}
