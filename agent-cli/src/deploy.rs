// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{ExternalRequirement, Intent, Plan};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentKind {
    Runtime,
    Platform,
}

impl DeploymentKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Platform => "platform",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiScope {
    Launcher,
    Arcade,
    All,
}

impl UiScope {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::Arcade => "arcade",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeploymentPlan {
    pub kind: DeploymentKind,
    pub profile: &'static str,
    pub ui_scope: UiScope,
    pub layout: &'static str,
    pub platform_components: Vec<&'static str>,
    pub changed_paths: Vec<PathBuf>,
}

impl DeploymentPlan {
    #[must_use]
    pub fn as_evidence_plan(&self, intent: Intent) -> Plan {
        Plan {
            intent,
            operations: Vec::new(),
            external_requirements: vec![ExternalRequirement {
                id: "deploy.execution-pending".into(),
                message: format!(
                    "{} deployment execution is not enabled yet",
                    self.kind.label()
                ),
            }],
        }
    }
}

pub fn plan(repository: &Path, mut paths: Vec<PathBuf>) -> Result<DeploymentPlan, String> {
    paths.sort();
    paths.dedup();
    let components = platform_components(&paths);
    if !components.is_empty() {
        require_platform_source_available(repository, &paths)?;
    }
    Ok(DeploymentPlan {
        kind: if components.is_empty() {
            DeploymentKind::Runtime
        } else {
            DeploymentKind::Platform
        },
        profile: "release-device",
        ui_scope: ui_scope(&paths),
        layout: "dev",
        platform_components: components,
        changed_paths: paths,
    })
}

fn platform_components(paths: &[PathBuf]) -> Vec<&'static str> {
    let mut components = Vec::new();
    if paths
        .iter()
        .any(|path| path.starts_with("mister/platform/kernel"))
    {
        components.push("kernel");
    }
    if paths
        .iter()
        .any(|path| path.starts_with("mister/platform/fpga"))
    {
        components.push("fpga");
    }
    if paths.iter().any(|path| {
        path.starts_with("mister/platform/runtime/main")
            || path.starts_with("docs/main-mister-fork.md")
            || path.starts_with(".github/workflows/platform-bundle.yml")
    }) {
        components.push("main");
    }
    components
}

fn ui_scope(paths: &[PathBuf]) -> UiScope {
    if paths.is_empty() {
        return UiScope::All;
    }
    if paths.iter().all(|path| {
        path.starts_with("apps/mister/src/ui_runner") || path.starts_with("apps/mister/ui/launcher")
    }) {
        UiScope::Launcher
    } else if paths.iter().all(|path| {
        path.starts_with("apps/mister/src/arcade") || path.starts_with("apps/mister/ui/arcade")
    }) {
        UiScope::Arcade
    } else {
        UiScope::All
    }
}

fn require_platform_source_available(repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all", "--"])
        .args(paths)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot inspect platform source state: {error}"))?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(
            "platform_source_unavailable: platform changes must be committed before CI deployment"
                .into(),
        );
    }
    let upstream = Command::new("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot resolve upstream branch: {error}"))?;
    if !upstream.status.success() {
        return Err("platform_source_unavailable: current branch has no upstream".into());
    }
    let ahead = Command::new("git")
        .args(["rev-list", "--count", "@{upstream}..HEAD"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot compare upstream branch: {error}"))?;
    if !ahead.status.success() || String::from_utf8_lossy(&ahead.stdout).trim() != "0" {
        return Err(
            "platform_source_unavailable: platform commit must be pushed before CI deployment"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_launcher_change_selects_canonical_launcher_build() {
        let paths = vec![PathBuf::from(
            "apps/mister/src/ui_runner/launcher_composition.rs",
        )];
        assert_eq!(platform_components(&paths), Vec::<&str>::new());
        assert_eq!(ui_scope(&paths), UiScope::Launcher);
    }

    #[test]
    fn unknown_or_empty_impact_falls_back_to_full_ui() {
        assert_eq!(ui_scope(&[]), UiScope::All);
        assert_eq!(
            ui_scope(&[PathBuf::from("docs/architecture.md")]),
            UiScope::All
        );
    }

    #[test]
    fn platform_components_are_in_stable_order() {
        let paths = vec![
            PathBuf::from("mister/platform/fpga/menu-vblank-latch/menu.sv"),
            PathBuf::from("mister/platform/kernel/scanout-slots/scanout.c"),
            PathBuf::from(".github/workflows/platform-bundle.yml"),
        ];
        assert_eq!(platform_components(&paths), vec!["kernel", "fpga", "main"]);
        assert_eq!(ui_scope(&paths), UiScope::All);
    }
}
