// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::build::BuildSpec;
use crate::platform_manifest::{self, Layout};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentKind {
    Runtime,
    Platform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryDecision {
    NoOp,
    Runtime,
    Platform,
}

impl DeliveryDecision {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NoOp => "no-op",
            Self::Runtime => "runtime",
            Self::Platform => "platform",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reconciliation {
    pub decision: DeliveryDecision,
    pub changed_paths: Vec<PathBuf>,
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

pub fn reconcile(repository: &Path, installed_manifest: &str, head: &str) -> Reconciliation {
    let Ok(installed) = platform_manifest::parse_installed(installed_manifest, Layout::Development)
    else {
        return conservative_reconciliation();
    };
    let installed_magik = installed.magik_revision();
    if installed_magik == head {
        return Reconciliation {
            decision: DeliveryDecision::NoOp,
            changed_paths: Vec::new(),
        };
    }
    let range = format!("{installed_magik}..{head}");
    let ancestor = Command::new("git")
        .args(["merge-base", "--is-ancestor", installed_magik, head])
        .current_dir(repository)
        .status()
        .is_ok_and(|status| status.success());
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=ACMRD", &range])
        .current_dir(repository)
        .output();
    let Ok(output) = output else {
        return conservative_reconciliation();
    };
    if !ancestor || !output.status.success() {
        return conservative_reconciliation();
    }
    let changed_paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let impact = changed_paths
        .iter()
        .map(|path| match crate::components::classify(path) {
            Some(crate::components::Component::Manager) => {
                crate::components::DeploymentImpact::Platform
            }
            Some(component) => component.deployment_impact(),
            None => crate::components::DeploymentImpact::Platform,
        })
        .max()
        .unwrap_or(crate::components::DeploymentImpact::None);
    let decision = match impact {
        crate::components::DeploymentImpact::None => DeliveryDecision::NoOp,
        crate::components::DeploymentImpact::Runtime => DeliveryDecision::Runtime,
        crate::components::DeploymentImpact::Platform => DeliveryDecision::Platform,
    };
    Reconciliation {
        decision,
        changed_paths,
    }
}

pub fn reconcile_with_platform(
    repository: &Path,
    installed_manifest: &str,
    head: &str,
    desired_release_tag: &str,
    desired_bundle_id: &str,
) -> Reconciliation {
    let Ok(installed) = platform_manifest::parse_installed(installed_manifest, Layout::Development)
    else {
        return conservative_reconciliation();
    };
    if installed.platform_release() != desired_release_tag
        || installed.platform_bundle_id() != desired_bundle_id
    {
        return conservative_reconciliation();
    }
    reconcile(repository, installed_manifest, head)
}

fn conservative_reconciliation() -> Reconciliation {
    Reconciliation {
        decision: DeliveryDecision::Platform,
        changed_paths: Vec::new(),
    }
}

pub fn deployment_paths(
    repository: &Path,
    explicit_paths: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    if !explicit_paths.is_empty() {
        return Ok(explicit_paths);
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
    fn runtime_launcher_change_selects_coherent_platform_installation() {
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
    fn exact_installed_revisions_are_a_no_op() {
        let revision = "a".repeat(40);
        let main = "b".repeat(40);
        let manifest = manifest(&revision, &main);
        assert_eq!(
            reconcile(Path::new("."), &manifest, &revision).decision,
            DeliveryDecision::NoOp
        );
    }

    #[test]
    fn invalid_manifest_is_conservatively_platform() {
        let revision = "a".repeat(40);
        assert_eq!(
            reconcile(Path::new("."), "invalid", &revision).decision,
            DeliveryDecision::Platform
        );
    }

    #[test]
    fn newer_published_platform_requires_platform_delivery() {
        let revision = "a".repeat(40);
        let installed = manifest(&revision, &"b".repeat(40));
        for (release_tag, bundle_id) in [
            ("platform-v0.17", "c".repeat(64)),
            ("platform-v0.16", "d".repeat(64)),
        ] {
            assert_eq!(
                reconcile_with_platform(
                    Path::new("."),
                    &installed,
                    &revision,
                    release_tag,
                    &bundle_id,
                )
                .decision,
                DeliveryDecision::Platform
            );
        }
    }

    #[test]
    fn current_published_platform_preserves_app_only_reconciliation() {
        let revision = "a".repeat(40);
        let installed = manifest(&revision, &"b".repeat(40));
        assert_eq!(
            reconcile_with_platform(
                Path::new("."),
                &installed,
                &revision,
                "platform-v0.16",
                &"e".repeat(64),
            )
            .decision,
            DeliveryDecision::NoOp
        );
    }

    #[test]
    fn partial_duplicate_extra_and_noncanonical_manifests_are_platform() {
        let revision = "a".repeat(40);
        let main = "b".repeat(40);
        for manifest in [
            format!(
                "format=mister-magik-platform-v3\nmagik_revision={revision}\nmain_revision={main}\n"
            ),
            format!("{}magik_revision={revision}\n", manifest(&revision, &main)),
            format!("{}extra=value\n", manifest(&revision, &main)),
            manifest(&revision, &main).replace(
                "/media/fat/mister-magik-dev/mister-magik-manager",
                "/tmp/manager",
            ),
        ] {
            assert_eq!(
                reconcile(Path::new("."), &manifest, &revision).decision,
                DeliveryDecision::Platform
            );
        }
    }

    #[test]
    fn accumulated_paths_select_no_op_runtime_and_platform() {
        let root = std::env::temp_dir().join(format!("agent-cli-reconcile-{}", std::process::id()));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("apps/mister/src")).unwrap();
        std::fs::create_dir_all(root.join("mister/platform/kernel")).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("README.md"), "base").unwrap();
        commit_all(&root, "base");
        let installed = git_value(&root, &["rev-parse", "HEAD"]);
        let main = "b".repeat(40);
        let installed_manifest = manifest(&installed, &main);

        std::fs::write(root.join("docs/note.md"), "docs").unwrap();
        commit_all(&root, "docs");
        let docs_head = git_value(&root, &["rev-parse", "HEAD"]);
        assert_eq!(
            reconcile(&root, &installed_manifest, &docs_head).decision,
            DeliveryDecision::NoOp
        );

        std::fs::write(root.join("apps/mister/src/runtime.rs"), "runtime").unwrap();
        commit_all(&root, "runtime");
        let runtime_head = git_value(&root, &["rev-parse", "HEAD"]);
        assert_eq!(
            reconcile(&root, &installed_manifest, &runtime_head).decision,
            DeliveryDecision::Runtime
        );

        std::fs::write(root.join("mister/platform/kernel/module.c"), "kernel").unwrap();
        commit_all(&root, "kernel");
        let platform_head = git_value(&root, &["rev-parse", "HEAD"]);
        assert_eq!(
            reconcile(&root, &installed_manifest, &platform_head).decision,
            DeliveryDecision::Platform
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn manifest(magik: &str, main: &str) -> String {
        let mut values = std::collections::BTreeMap::<String, String>::new();
        values.insert("format".into(), "mister-magik-platform-v3".to_owned());
        values.insert("platform_release".into(), "platform-v0.16".to_owned());
        values.insert("platform_release_number".into(), "16".to_owned());
        values.insert("platform_bundle_id".into(), "e".repeat(64));
        values.insert("latch_protocol_version".into(), "5".to_owned());
        values.insert("latch_capability_mask".into(), "0x03ff".to_owned());
        for (name, path) in Layout::Development.paths() {
            values.insert(format!("{name}_path"), path.to_owned());
        }
        for name in [
            "main_sha256",
            "gui_sha256",
            "manager_sha256",
            "scanout_module_sha256",
            "scanout_metadata_sha256",
            "latch_rbf_sha256",
            "latch_metadata_sha256",
            "platform_contract_sha256",
        ] {
            values.insert(name.into(), "c".repeat(64));
        }
        values.insert("main_revision".into(), main.to_owned());
        values.insert("magik_revision".into(), magik.to_owned());
        values.insert("menu_revision".into(), "d".repeat(40));
        values.insert(
            "qualification_candidate_id".into(),
            platform_manifest::qualification_candidate_id(&values),
        );
        platform_manifest::FIELDS
            .iter()
            .map(|field| format!("{field}={}\n", values[*field]))
            .collect()
    }

    fn git(repository: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(repository)
                .status()
                .unwrap()
                .success()
        );
    }

    fn git_value(repository: &Path, args: &[&str]) -> String {
        String::from_utf8(
            Command::new("git")
                .args(args)
                .current_dir(repository)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into()
    }

    fn commit_all(repository: &Path, message: &str) {
        git(repository, &["add", "."]);
        git(
            repository,
            &[
                "-c",
                "user.name=Agent CLI",
                "-c",
                "user.email=agent@example.invalid",
                "commit",
                "-q",
                "-m",
                message,
            ],
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
    fn canonical_build_uses_the_dormant_production_profiler() {
        let build = BuildSpec::canonical(UiScope::Launcher);
        assert_eq!(build.features(), ["ui", "profile"]);
        assert_eq!(build.ui_scope(), UiScope::Launcher);
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
            "build_receipt_tsv\tbinary_sha256=abc\tprofile=release-device\tfeatures=ui\tui_scope=all\tbuild_number=2429\tversion=0.2.2429\tsource_commit=deadbeef\tsource_dirty=1\tcache_identity=v3\tlock_sha256=lock\ttoolchain_sha256=toolchain\n",
        )
        .unwrap();
        assert_eq!(receipt.profile, "release-device");
        assert_eq!(receipt.features, "ui");
        assert_eq!(receipt.ui_scope, "all");
        assert_eq!(receipt.build_number, "2429");
        assert_eq!(receipt.version, "0.2.2429");
        assert!(receipt.source_dirty);
        assert!(
            crate::build::BuildReceipt::parse("build_receipt_tsv\tprofile=release-device").is_err()
        );
    }
}
