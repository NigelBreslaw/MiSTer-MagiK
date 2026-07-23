// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::BuildSpec;
use crate::model::{ActionKind, Intent, Operation, Plan, Risk, WorkflowPhase};
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
    pub build: BuildSpec,
    pub platform_candidate: Option<crate::platform_ci::Candidate>,
}

impl DeploymentPlan {
    #[must_use]
    pub fn as_evidence_plan(&self, intent: Intent) -> Plan {
        let mut inputs = vec![
            format!("kind={}", self.kind.label()),
            format!("profile={}", self.profile),
            format!("ui_scope={}", self.ui_scope.label()),
            format!("layout={}", self.layout),
            format!("artifact={}", self.build.artifact().display()),
        ];
        inputs.extend(
            self.platform_components
                .iter()
                .map(|value| format!("component={value}")),
        );
        inputs.extend(
            self.changed_paths
                .iter()
                .map(|path| format!("changed={}", path.display())),
        );
        if let Some(candidate) = &self.platform_candidate {
            inputs.extend([
                format!("ci_run_id={}", candidate.run_id),
                format!("ci_head_sha={}", candidate.head_sha),
                format!("ci_head_branch={}", candidate.head_branch),
                format!("bundle_id={}", candidate.bundle_id),
                format!("main_identity={}", candidate.main_identity),
                format!("fpga_identity={}", candidate.fpga_identity),
                format!("kernel_identity={}", candidate.kernel_identity),
                format!("ci_archive={}", candidate.archive.display()),
                format!("ci_manifest={}", candidate.manifest.display()),
            ]);
        }
        Plan {
            intent,
            operations: vec![Operation {
                id: format!("deploy.{}", self.kind.label()),
                title: format!("Deploy {} installation", self.kind.label()),
                risk: Risk::DeviceWrite,
                action: ActionKind::DeviceTransaction,
                phase: WorkflowPhase::Device,
                program: "scripts/agent".into(),
                args: vec!["deploy".into()],
                reason: "inferred deployment transaction".into(),
                failure_hint: "inspect the recorded deployment phases and rollback result".into(),
                inputs,
                builtin: None,
            }],
            external_requirements: Vec::new(),
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
    let ui_scope = ui_scope(&paths);
    Ok(DeploymentPlan {
        kind: if !components.is_empty() {
            DeploymentKind::Platform
        } else {
            DeploymentKind::Runtime
        },
        profile: "release-device",
        ui_scope,
        layout: "dev",
        platform_components: components,
        changed_paths: paths,
        build: BuildSpec::canonical(ui_scope),
        platform_candidate: None,
    })
}

pub fn deployment_paths(
    repository: &Path,
    task_paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    if !task_paths.is_empty() {
        return Ok(task_paths);
    }
    let output = Command::new("git")
        .args(["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot inspect deployed commit: {error}"))?;
    if !output.status.success() {
        return Err("cannot inspect paths from the current commit".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
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
    fn runtime_launcher_change_selects_runtime_installation() {
        let paths = vec![PathBuf::from(
            "apps/mister/src/ui_runner/launcher_composition.rs",
        )];
        assert_eq!(platform_components(&paths), Vec::<&str>::new());
        assert_eq!(ui_scope(&paths), UiScope::Launcher);
        assert_eq!(
            plan(Path::new("."), paths).unwrap().kind,
            DeploymentKind::Runtime
        );
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
        ];
        assert_eq!(platform_components(&paths), vec!["kernel", "fpga"]);
        assert_eq!(ui_scope(&paths), UiScope::All);
    }

    #[test]
    fn documentation_and_workflow_changes_do_not_rebuild_platform_components() {
        assert_eq!(
            platform_components(&[
                PathBuf::from("docs/main-mister-fork.md"),
                PathBuf::from(".github/workflows/platform-bundle.yml"),
            ]),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn canonical_build_never_adds_non_production_features() {
        let build = BuildSpec::canonical(UiScope::Launcher);
        assert_eq!(build.features(), ["ui"]);
        assert_eq!(build.ui_scope(), UiScope::Launcher);
    }

    #[test]
    fn evidence_plan_preserves_deployment_risk_and_identity() {
        let deployment = plan(
            Path::new("."),
            vec![PathBuf::from("apps/mister/src/launcher.rs")],
        )
        .unwrap();
        let plan = deployment.as_evidence_plan(Intent::Deliver);
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].risk, Risk::DeviceWrite);
        assert!(plan.operations[0]
            .inputs
            .iter()
            .any(|input| input == "kind=runtime"));
        assert!(plan.operations[0]
            .inputs
            .iter()
            .any(|input| input == "changed=apps/mister/src/launcher.rs"));
    }

    #[test]
    fn platform_plan_refuses_unpublished_source() {
        let root =
            std::env::temp_dir().join(format!("agent-cli-platform-plan-{}", std::process::id()));
        let repository = root.join("repo");
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::create_dir_all(root.join("Main_MiSTer")).unwrap();
        let error = plan(
            &repository,
            vec![PathBuf::from(
                "mister/platform/fpga/menu-vblank-latch/menu.sv",
            )],
        )
        .unwrap_err();
        assert!(error.contains("platform changes must be committed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn build_receipt_requires_canonical_identity_and_provenance() {
        let receipt = crate::build::BuildReceipt::parse(
            "build_receipt_tsv\tbinary_sha256=abc\tprofile=release-device\tfeatures=ui\tui_scope=all\tsource_commit=deadbeef\tsource_dirty=1\tcache_identity=v3\tlock_sha256=lock\ttoolchain_sha256=toolchain\n",
        )
        .unwrap();
        assert_eq!(receipt.profile, "release-device");
        assert_eq!(receipt.features, "ui");
        assert_eq!(receipt.ui_scope, "all");
        assert!(receipt.source_dirty);
        assert!(
            crate::build::BuildReceipt::parse("build_receipt_tsv\tprofile=release-device").is_err()
        );
    }
}
