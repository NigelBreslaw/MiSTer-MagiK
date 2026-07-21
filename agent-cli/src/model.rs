// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    LocalWrite,
    DeviceWrite,
    Destructive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Task(String),
    Staged,
    WorkingTree,
    Paths(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    TaskBegin { task_id: String, replace: bool },
    TaskStatus { task_id: String },
    Commit { task_id: String, message: String },
    Plan { scope: Scope, verbose: bool },
    Check { scope: Scope },
    Verify { scope: Scope },
    ListRuns { failed: bool, recent: usize },
    ShowRun { run_id: String },
    ReviewScripts,
    DatabaseStatus,
    PruneLogs,
    Interactive,
    Doctor,
    Deploy { task_id: String },
    DeployRecipe { recipe: String },
}

impl Intent {
    #[must_use]
    pub const fn risk(&self) -> Risk {
        match self {
            Self::Commit { .. } => Risk::LocalWrite,
            Self::Deploy { .. } | Self::DeployRecipe { .. } => Risk::DeviceWrite,
            _ => Risk::ReadOnly,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub title: String,
    pub risk: Risk,
    pub program: String,
    pub args: Vec<String>,
    pub reason: String,
    pub failure_hint: String,
    #[serde(default)]
    pub inputs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub intent: Intent,
    pub operations: Vec<Operation>,
    pub external_requirements: Vec<ExternalRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalRequirement {
    pub id: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Rejected,
    NoOp,
    ExternalRequired,
}
