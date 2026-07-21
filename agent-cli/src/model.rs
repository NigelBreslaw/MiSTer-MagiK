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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Cpu,
    Cargo,
    AppleContainer,
    GitIndex,
    Network,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Cargo,
    Script,
    AppleContainer,
    Git,
    PlatformCi,
    DeviceTransaction,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Cheap,
    Host,
    Expensive,
    External,
    Device,
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
    DatabaseStatus,
    PruneLogs,
    Interactive,
    Doctor,
    Diagnose,
    DisplayMode { video_mode: String },
    Deliver { task_id: String },
    Benchmark { task_id: String },
    ReleaseQualify,
    Build { intent: crate::build::BuildIntent },
    DeployRecipe { recipe: String },
}

impl Intent {
    #[must_use]
    pub const fn risk(&self) -> Risk {
        match self {
            Self::Commit { .. } | Self::Verify { .. } => Risk::LocalWrite,
            Self::ReleaseQualify => Risk::Destructive,
            Self::Deliver { .. }
            | Self::Benchmark { .. }
            | Self::Diagnose
            | Self::DisplayMode { .. }
            | Self::DeployRecipe { .. } => Risk::DeviceWrite,
            Self::Build { .. } => Risk::LocalWrite,
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

impl Operation {
    #[must_use]
    pub fn action_kind(&self) -> ActionKind {
        if self.risk >= Risk::DeviceWrite {
            ActionKind::DeviceTransaction
        } else if self.program == "cargo" {
            ActionKind::Cargo
        } else if self.program == "git" {
            ActionKind::Git
        } else if self.id.starts_with("arm.") || self.args.first().is_some_and(|arg| arg == "build")
        {
            ActionKind::AppleContainer
        } else if self.program == "gh" {
            ActionKind::PlatformCi
        } else {
            ActionKind::Script
        }
    }

    #[must_use]
    pub fn resource_class(&self) -> ResourceClass {
        match self.action_kind() {
            ActionKind::Cargo => ResourceClass::Cargo,
            ActionKind::AppleContainer => ResourceClass::AppleContainer,
            ActionKind::Git => ResourceClass::GitIndex,
            ActionKind::PlatformCi => ResourceClass::Network,
            ActionKind::DeviceTransaction => ResourceClass::Device,
            ActionKind::Script => ResourceClass::Cpu,
        }
    }

    #[must_use]
    pub fn workflow_phase(&self) -> WorkflowPhase {
        match self.resource_class() {
            ResourceClass::Device => WorkflowPhase::Device,
            ResourceClass::Network => WorkflowPhase::External,
            ResourceClass::AppleContainer => WorkflowPhase::Expensive,
            ResourceClass::Cargo => WorkflowPhase::Host,
            ResourceClass::Cpu | ResourceClass::GitIndex => {
                if self.id.contains("format")
                    || self.id.contains("syntax")
                    || self.id == "repo.diff-check"
                {
                    WorkflowPhase::Cheap
                } else {
                    WorkflowPhase::Host
                }
            }
        }
    }
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
