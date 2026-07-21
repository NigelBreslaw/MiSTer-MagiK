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
    pub build: BuildSpec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildSpec {
    pub program: &'static str,
    pub args: Vec<String>,
    pub artifact: PathBuf,
    pub receipt: PathBuf,
    pub profile: &'static str,
    pub features: &'static str,
    pub ui_scope: UiScope,
}

impl BuildSpec {
    #[must_use]
    pub fn canonical(ui_scope: UiScope) -> Self {
        let artifact = PathBuf::from(
            "apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb",
        );
        Self {
            program: "apps/mister/build-arm.sh",
            args: vec![
                "--device".into(),
                "--ui-scope".into(),
                ui_scope.label().into(),
            ],
            receipt: PathBuf::from(format!("{}.build-receipt.tsv", artifact.display())),
            artifact,
            profile: "release-device",
            features: "ui",
            ui_scope,
        }
    }

    pub fn verify(&self, repository: &Path) -> Result<BuildReceipt, String> {
        let artifact = repository.join(&self.artifact);
        let receipt_path = repository.join(&self.receipt);
        if !artifact.is_file() {
            return Err(format!("build artifact is missing: {}", artifact.display()));
        }
        let receipt = std::fs::read_to_string(&receipt_path).map_err(|error| {
            format!(
                "cannot read build receipt {}: {error}",
                receipt_path.display()
            )
        })?;
        let receipt = BuildReceipt::parse(&receipt)?;
        if receipt.profile != self.profile
            || receipt.features != self.features
            || receipt.ui_scope != self.ui_scope.label()
        {
            return Err("build receipt does not match the inferred canonical build".into());
        }
        let output = Command::new("shasum")
            .args(["-a", "256"])
            .arg(&artifact)
            .output()
            .map_err(|error| format!("cannot hash build artifact: {error}"))?;
        let actual = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        if !output.status.success() || actual != receipt.binary_sha256 {
            return Err("build artifact checksum does not match its receipt".into());
        }
        Ok(receipt)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildReceipt {
    pub binary_sha256: String,
    pub profile: String,
    pub features: String,
    pub ui_scope: String,
    pub source_commit: String,
    pub source_dirty: bool,
}

impl BuildReceipt {
    fn parse(text: &str) -> Result<Self, String> {
        let fields: std::collections::BTreeMap<_, _> = text
            .trim()
            .split('\t')
            .skip(1)
            .filter_map(|field| field.split_once('='))
            .collect();
        let required = |name: &str| {
            fields
                .get(name)
                .map(|value| (*value).to_owned())
                .ok_or_else(|| format!("build receipt is missing {name}"))
        };
        let source_dirty = match required("source_dirty")?.as_str() {
            "0" => false,
            "1" => true,
            _ => return Err("build receipt has invalid source_dirty".into()),
        };
        Ok(Self {
            binary_sha256: required("binary_sha256")?,
            profile: required("profile")?,
            features: required("features")?,
            ui_scope: required("ui_scope")?,
            source_commit: required("source_commit")?,
            source_dirty,
        })
    }
}

impl DeploymentPlan {
    #[must_use]
    pub fn as_evidence_plan(&self, intent: Intent) -> Plan {
        let external_requirements = if self.kind == DeploymentKind::Platform {
            vec![ExternalRequirement {
                id: "deploy.platform-execution-pending".into(),
                message: "platform deployment execution is not enabled yet".into(),
            }]
        } else {
            Vec::new()
        };
        Plan {
            intent,
            operations: Vec::new(),
            external_requirements,
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
        kind: if components.is_empty() {
            DeploymentKind::Runtime
        } else {
            DeploymentKind::Platform
        },
        profile: "release-device",
        ui_scope,
        layout: "dev",
        platform_components: components,
        changed_paths: paths,
        build: BuildSpec::canonical(ui_scope),
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

    #[test]
    fn canonical_build_never_adds_non_production_features() {
        let build = BuildSpec::canonical(UiScope::Launcher);
        assert_eq!(build.args, vec!["--device", "--ui-scope", "launcher"]);
        assert_eq!(build.features, "ui");
        assert!(!build.args.iter().any(|arg| matches!(
            arg.as_str(),
            "--fast" | "--bench-tools" | "--diagnostics" | "--experiments"
        )));
    }

    #[test]
    fn build_receipt_requires_canonical_identity_and_provenance() {
        let receipt = BuildReceipt::parse(
            "build_receipt_tsv\tbinary_sha256=abc\tprofile=release-device\tfeatures=ui\tui_scope=all\tsource_commit=deadbeef\tsource_dirty=1\n",
        )
        .unwrap();
        assert_eq!(receipt.profile, "release-device");
        assert_eq!(receipt.features, "ui");
        assert_eq!(receipt.ui_scope, "all");
        assert!(receipt.source_dirty);
        assert!(BuildReceipt::parse("build_receipt_tsv\tprofile=release-device").is_err());
    }
}
