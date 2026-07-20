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
    Staged,
    WorkingTree,
    Paths(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Plan { scope: Scope },
    Check { scope: Scope },
    Verify { scope: Scope },
    ListRuns { failed: bool, recent: usize },
    ShowRun { run_id: String },
    ReviewScripts,
    DatabaseStatus,
    PruneLogs,
    Interactive,
    VerifyFullHost,
    Doctor,
    Rust { task: RustTask },
    HostTools { full: bool },
    ReleaseHost,
    Arm { task: ArmTask },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RustTask {
    Format,
    Test,
    Check,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmTask {
    CheckLib,
    CheckLauncher,
    CheckArcade,
    CheckAll,
    BuildDevice,
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub intent: Intent,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Rejected,
    NoOp,
}
