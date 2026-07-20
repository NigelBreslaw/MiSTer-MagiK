// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Intent, Operation, Plan, Risk};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[must_use]
pub fn lint_plan(intent: Intent, paths: Vec<PathBuf>) -> Plan {
    let unique: BTreeSet<_> = paths.into_iter().collect();
    let mut operations = Vec::new();
    if unique.iter().any(|path| path.starts_with("agent-cli")) {
        operations.extend(agent_cli_operations());
    }
    let repository_paths: Vec<_> = unique
        .iter()
        .filter(|path| !path.starts_with("agent-cli") && relevant(path))
        .collect();
    if !repository_paths.is_empty() {
        let mut args = vec!["paths".into()];
        args.extend(
            repository_paths
                .iter()
                .map(|path| path.display().to_string()),
        );
        operations.push(Operation {
            id: "repo.affected-lint".into(),
            title: "Validate affected repository paths".into(),
            risk: Risk::ReadOnly,
            program: "scripts/validate".into(),
            args,
        });
    }
    Plan { intent, operations }
}

#[must_use]
pub fn workflow_plan(intent: Intent) -> Plan {
    use crate::model::RustTask;
    let operation = match &intent {
        Intent::VerifyFullHost => operation(
            "verify.full-host",
            "Run complete host verification",
            "scripts/validate",
            &["full-host"],
        ),
        Intent::Doctor => operation(
            "doctor.full-host",
            "Inspect host prerequisites",
            "scripts/doctor",
            &["--scope", "full-host"],
        ),
        Intent::Rust { task } => {
            let argument = match task {
                RustTask::Format => "fmt",
                RustTask::Test => "test",
                RustTask::Check => "check",
            };
            operation(
                "rust.workflow",
                "Run Rust workflow",
                "scripts/dev-rust",
                &[argument],
            )
        }
        Intent::HostTools { full } => operation(
            "host-tools",
            "Run host tool checks",
            "scripts/test-host-tools.sh",
            &[if *full { "--full" } else { "--fast" }],
        ),
        Intent::ReleaseHost => operation(
            "release.host",
            "Run host release gate",
            "scripts/release-check-host.sh",
            &[],
        ),
        _ => {
            return Plan {
                intent,
                operations: Vec::new(),
            }
        }
    };
    Plan {
        intent,
        operations: vec![operation],
    }
}

fn operation(id: &str, title: &str, program: &str, args: &[&str]) -> Operation {
    Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
    }
}

fn agent_cli_operations() -> Vec<Operation> {
    [
        (
            "agent-cli.format",
            "Check agent-cli formatting",
            vec!["fmt", "--manifest-path", "agent-cli/Cargo.toml", "--check"],
        ),
        (
            "agent-cli.test",
            "Test agent-cli",
            vec![
                "test",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "--offline",
            ],
        ),
        (
            "agent-cli.clippy",
            "Lint agent-cli",
            vec![
                "clippy",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "--all-targets",
                "--offline",
                "--",
                "-D",
                "warnings",
            ],
        ),
    ]
    .into_iter()
    .map(|(id, title, args)| Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
        program: "cargo".into(),
        args: args.into_iter().map(String::from).collect(),
    })
    .collect()
}

fn relevant(path: &Path) -> bool {
    [
        "apps",
        "crates",
        "mister",
        "scripts",
        "docs",
        "documentation",
        ".github",
        ".githooks",
    ]
    .iter()
    .any(|root| path.starts_with(root))
        || matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(
                "AGENTS.md" | "Cargo.toml" | "Cargo.lock" | "rust-toolchain.toml" | "rustfmt.toml"
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;

    #[test]
    fn deduplicates_overlapping_agent_cli_paths() {
        let intent = Intent::PlanLint {
            scope: Scope::Paths(vec![]),
        };
        let plan = lint_plan(
            intent,
            vec![
                "agent-cli/src/main.rs".into(),
                "agent-cli/src/main.rs".into(),
            ],
        );
        assert_eq!(plan.operations.len(), 3);
    }

    #[test]
    fn unrelated_paths_are_a_no_op() {
        let intent = Intent::Lint {
            scope: Scope::Paths(vec![]),
        };
        assert!(lint_plan(intent, vec!["README.md".into()])
            .operations
            .is_empty());
    }
}
