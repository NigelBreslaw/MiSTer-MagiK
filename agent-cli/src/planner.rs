// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{ActionKind, ExternalRequirement, Intent, Operation, Plan, Risk, WorkflowPhase};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Depth {
    Check,
    Verify,
}

pub fn affected_plan(intent: Intent, paths: Vec<PathBuf>) -> Result<Plan, String> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("agent-cli must live below the repository root");
    affected_plan_at(repository, intent, paths)
}

pub fn affected_plan_at(
    repository: &Path,
    intent: Intent,
    paths: Vec<PathBuf>,
) -> Result<Plan, String> {
    let depth = if matches!(intent, Intent::Verify { .. }) {
        Depth::Verify
    } else {
        Depth::Check
    };
    let paths: BTreeSet<_> = paths.into_iter().collect();
    let unclassified: Vec<_> = paths
        .iter()
        .filter(|path| !classified(path))
        .map(|path| path.display().to_string())
        .collect();
    if !unclassified.is_empty() {
        return Err(format!(
            "unclassified task paths: {}; add them to the typed impact map",
            unclassified.join(", ")
        ));
    }
    let mut operations = BTreeMap::new();
    let mut external_requirements = Vec::new();
    for path in &paths {
        add_path_operations(repository, path, depth, &mut operations);
        if path.starts_with("mister/platform/fpga") {
            external_requirements.push(rbf_external_requirement());
        }
    }
    external_requirements.sort_by(|left, right| left.id.cmp(&right.id));
    external_requirements.dedup_by(|left, right| left.id == right.id);
    let mut operations: Vec<_> = operations.into_values().collect();
    operations.sort_by(|left, right| {
        left.workflow_phase()
            .cmp(&right.workflow_phase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(Plan {
        intent,
        operations,
        external_requirements,
    })
}

fn classified(path: &Path) -> bool {
    crate::components::classify(path).is_some()
}

fn is_root_file(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
}

fn rbf_external_requirement() -> ExternalRequirement {
    ExternalRequirement {
        id: "github-actions.rbf-build".into(),
        message: "External validation required: the RBF can only be built by the ‘Build MiSTer MagiK Platform’ GitHub Actions workflow.\n\nLocal Quartus or RBF builds are prohibited on macOS.\nUnder no circumstances attempt a local RBF build.\nReport this requirement to the user.".into(),
    }
}

fn add_path_operations(
    repository: &Path,
    path: &Path,
    depth: Depth,
    out: &mut BTreeMap<String, Operation>,
) {
    let default_input = path.display().to_string();
    let mut add = |mut operation: Operation| {
        if operation.inputs.is_empty() {
            operation.inputs.push(default_input.clone());
        }
        out.entry(operation.id.clone()).or_insert(operation);
    };
    if path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md")
        || path.starts_with("docs/agents")
    {
        add(op(
            "repo.guidance",
            "Check agent guidance",
            "python3",
            &["scripts/checks/check-agent-guidance.py"],
            "agent guidance changed",
        ));
    }
    if path.starts_with(".codex")
        || path.starts_with(".lspi")
        || path == Path::new("scripts/rust-analyzer")
        || path == Path::new("apps/mister/rust-toolchain.toml")
    {
        add(op(
            "host.doctor-tests",
            "Test host doctor",
            "python3",
            &["scripts/tests/test-doctor.py"],
            "semantic tooling changed → doctor contract",
        ));
    }
    if is_root_file(path)
        || path.starts_with("LICENSES")
        || path.starts_with("history")
        || path.starts_with("private")
    {
        add(diff_check());
    }
    if path.starts_with("agent-cli") {
        add(with_inputs(
            cargo_format(
                "agent-cli.format",
                "Check agent-cli formatting",
                &["fmt", "--manifest-path", "agent-cli/Cargo.toml", "--check"],
                "agent-cli source → formatter",
            ),
            &["agent-cli"],
        ));
        add(with_inputs(
            cargo(
                "agent-cli.tests",
                "Test agent-cli",
                &["test", "--manifest-path", "agent-cli/Cargo.toml"],
                "agent-cli source → unit tests",
            ),
            &["agent-cli"],
        ));
        if depth == Depth::Verify {
            add(with_inputs(
                cargo(
                    "agent-cli.clippy",
                    "Lint agent-cli",
                    &[
                        "clippy",
                        "--manifest-path",
                        "agent-cli/Cargo.toml",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    "agent-cli source → clippy",
                ),
                &["agent-cli"],
            ));
        }
    }
    if path.starts_with("crates/agent-protocol") {
        add(with_inputs(
            cargo(
                "protocol.host-consumer",
                "Check host protocol consumer",
                &["check", "--manifest-path", "mister/tools/host/Cargo.toml"],
                "agent protocol → host consumer",
            ),
            &["crates/agent-protocol", "mister/tools/host"],
        ));
        add(with_inputs(
            cargo(
                "protocol.agent-consumer",
                "Check device-agent protocol consumer",
                &["check", "--manifest-path", "mister/tools/agent/Cargo.toml"],
                "agent protocol → device-agent consumer",
            ),
            &["crates/agent-protocol", "mister/tools/agent"],
        ));
    }
    if path.starts_with("crates/catalog") {
        add(with_inputs(
            cargo_format(
                "catalog.format",
                "Check catalog formatting",
                &[
                    "fmt",
                    "--manifest-path",
                    "crates/catalog/Cargo.toml",
                    "--check",
                ],
                "catalog source → formatter",
            ),
            &["crates/catalog"],
        ));
        add(with_inputs(
            cargo(
                "catalog.builder-tests",
                "Test catalog builder",
                &[
                    "test",
                    "--manifest-path",
                    "crates/catalog/Cargo.toml",
                    "--features",
                    "builder",
                ],
                "catalog source → builder tests",
            ),
            &["crates/catalog"],
        ));
        add(with_inputs(
            cargo(
                "catalog.reader-check",
                "Check catalog reader",
                &[
                    "check",
                    "--manifest-path",
                    "crates/catalog/Cargo.toml",
                    "--no-default-features",
                    "--features",
                    "reader",
                ],
                "catalog source → reader check",
            ),
            &["crates/catalog"],
        ));
        if depth == Depth::Verify {
            add(with_inputs(
                cargo(
                    "catalog.clippy",
                    "Lint catalog",
                    &[
                        "clippy",
                        "--manifest-path",
                        "crates/catalog/Cargo.toml",
                        "--all-features",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                    "catalog source → clippy",
                ),
                &["crates/catalog"],
            ));
        }
    }
    if path.starts_with("apps/mister") {
        add(diff_check());
        if repository.join(path).exists()
            && path.extension().and_then(|extension| extension.to_str()) == Some("sh")
        {
            let text = path.to_string_lossy();
            let id = format!("app.script-syntax.{}", text.replace(['/', '.'], "-"));
            add(op_owned(
                &id,
                &format!("Check {} syntax", path.display()),
                "bash",
                vec!["-n".into(), text.to_string()],
                "MiSTer build script → syntax",
            ));
        }
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("AGENTS.md")
        && (path.starts_with("apps/mister/src")
            || path.starts_with("apps/mister/ui")
            || path.starts_with("apps/mister/ui-generated")
            || path.starts_with("apps/mister/examples")
            || path.starts_with("apps/mister/.cargo")
            || matches!(
                path.to_str(),
                Some(
                    "apps/mister/Cargo.toml"
                        | "apps/mister/Cargo.lock"
                        | "apps/mister/build.rs"
                        | "apps/mister/rust-toolchain.toml"
                        | "apps/mister/Cross.toml"
                        | "apps/mister/Dockerfile.cross-armv7"
                )
            ))
    {
        add(cargo_format(
            "app.format",
            "Check MiSTer app formatting",
            &[
                "fmt",
                "--manifest-path",
                "apps/mister/Cargo.toml",
                "--check",
            ],
            "MiSTer app source → formatter",
        ));
        if path.starts_with("apps/mister/src/ui_runner") {
            add(cargo(
                "app.ui-tests",
                "Test launcher binary logic",
                &[
                    "test",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--bin",
                    "mister-magik-fb",
                    "--no-default-features",
                    "--features",
                    "ui",
                    "launcher_catalog_session::tests",
                ],
                "launcher session source → focused UI binary tests",
            ));
        } else {
            add(cargo(
                "app.tests",
                "Test MiSTer host logic",
                &[
                    "test",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--lib",
                    "--no-default-features",
                ],
                "MiSTer app source → host tests",
            ));
        }
        if depth == Depth::Verify {
            add(cargo(
                "app.clippy",
                "Lint MiSTer host logic",
                &[
                    "clippy",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--lib",
                    "--no-default-features",
                    "--",
                    "-D",
                    "warnings",
                ],
                "MiSTer app source → clippy",
            ));
            add(cargo(
                "app.ui-check",
                "Check production UI",
                &[
                    "check",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--bin",
                    "mister-magik-fb",
                    "--no-default-features",
                    "--features",
                    "ui",
                ],
                "MiSTer app source → production UI check",
            ));
            if path.starts_with("apps/mister/ui") || path.starts_with("apps/mister/src/ui_runner") {
                add(apple_container(op(
                    "arm.check-launcher",
                    "Check launcher in Apple container",
                    "scripts/agent",
                    &["build", "validate-launcher"],
                    "launcher source → ARM validation",
                )));
            } else {
                add(apple_container(op(
                    "arm.check-lib",
                    "Check library in Apple container",
                    "scripts/agent",
                    &["build", "validate-library"],
                    "MiSTer source → ARM validation",
                )));
            }
        }
    }
    if path.starts_with("mister/platform/kernel") {
        add(op(
            "kernel.workflow-contract",
            "Test kernel scanout workflow",
            "python3",
            &["scripts/tests/test-kernel-scanout-workflows.py"],
            "kernel source → workflow contract",
        ));
    }
    if path.starts_with("mister/platform/fpga") {
        add(op(
            "fpga.workflow-contract",
            "Test platform workflow",
            "python3",
            &["scripts/tests/test-platform-bundle-workflow.py"],
            "FPGA source → workflow contract",
        ));
    }
    if path == Path::new("tools/host-camera-native.swift") {
        add(op(
            "tools.host-camera-typecheck",
            "Type-check native host camera helper",
            "swiftc",
            &["-typecheck", "tools/host-camera-native.swift"],
            "native host-camera source changed",
        ));
    }
    if path.starts_with("scripts") {
        add_script_operations(repository, path, depth, &mut add);
    }
    if path.starts_with(".github") || path.starts_with(".githooks") {
        add(op(
            "repo.workflow-contract",
            "Check workflow contracts",
            "python3",
            &["scripts/tests/test-ci-cache-contract.py"],
            "workflow configuration changed",
        ));
    }
    if path.starts_with("docs") && !path.starts_with("docs/agents") {
        add(diff_check());
    }
    if path.starts_with("documentation") {
        add(op(
            "documentation.build",
            "Build documentation",
            "corepack",
            &["pnpm", "--dir", "documentation", "run", "build"],
            "documentation source → site build",
        ));
    }
    if path.starts_with("apps/desktop") {
        add(cargo(
            "desktop.tests",
            "Test desktop application",
            &["test", "--manifest-path", "apps/desktop/Cargo.toml"],
            "desktop source → tests",
        ));
        if depth == Depth::Verify {
            add(cargo(
                "desktop.check",
                "Check compiled desktop UI",
                &[
                    "check",
                    "--manifest-path",
                    "apps/desktop/Cargo.toml",
                    "--no-default-features",
                    "--features",
                    "compiled-ui",
                ],
                "desktop source → compiled UI",
            ));
        }
    }
    add_crate(path, "crates/magik-core", "magik-core", depth, out);
    add_crate(
        path,
        "crates/framebuffer-stream",
        "framebuffer-stream",
        depth,
        out,
    );
    add_crate(path, "crates/agent-protocol", "agent-protocol", depth, out);
    add_crate(path, "crates/media-contract", "media-contract", depth, out);
    add_crate(
        path,
        "mister/platform/runtime",
        "mister-runtime",
        depth,
        out,
    );
    add_crate(
        path,
        "mister/platform/contracts/latch",
        "latch-contract",
        depth,
        out,
    );
    add_crate(
        path,
        "mister/platform/contracts/scanout",
        "scanout-contract",
        depth,
        out,
    );
    add_crate(path, "mister/tools/host", "mister-host", depth, out);
    add_crate(path, "mister/tools/agent", "mister-agent", depth, out);
}

fn add_script_operations(
    repository: &Path,
    path: &Path,
    _depth: Depth,
    add: &mut impl FnMut(Operation),
) {
    let text = path.to_string_lossy();
    add(op(
        "scripts.licenses",
        "Check script license headers",
        "python3",
        &["scripts/checks/check-license-headers.py"],
        "script source → license contract",
    ));
    if repository.join(path).exists()
        && (path.extension().and_then(|extension| extension.to_str()) == Some("sh")
            || matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("mister" | "rust-analyzer")
            ))
    {
        let id = format!("script.syntax.{}", text.replace(['/', '.'], "-"));
        add(op_owned(
            &id,
            &format!("Check {} syntax", path.display()),
            "bash",
            vec!["-n".into(), text.to_string()],
            "changed shell script → syntax",
        ));
    }
    add(op(
        "scripts.no-orchestrator-regrowth",
        "Check operational shell boundaries",
        "python3",
        &["scripts/checks/check-no-operational-shell-orchestrators.py"],
        "script source → orchestration ownership contract",
    ));
    if text.contains("platform-bundle") || text.contains("platform-artifact") {
        add(op(
            "scripts.platform-workflow",
            "Test platform workflow",
            "python3",
            &["scripts/tests/test-platform-bundle-workflow.py"],
            "platform tooling changed",
        ));
        add(op(
            "scripts.platform-selection",
            "Test platform artifact selection",
            "python3",
            &["scripts/tests/test-platform-artifact-selection.py"],
            "platform tooling changed",
        ));
    }
    if text.contains("install") || text.contains("distribution") || text.contains("package-") {
        add(op(
            "scripts.distribution",
            "Test distribution workflow",
            "python3",
            &["scripts/tests/test-distribution-workflow.py"],
            "packaging tooling changed",
        ));
    }
}

fn add_crate(
    path: &Path,
    root: &str,
    id: &str,
    depth: Depth,
    out: &mut BTreeMap<String, Operation>,
) {
    if !path.starts_with(root) {
        return;
    }
    let manifest = format!("{root}/Cargo.toml");
    for mut operation in [
        cargo_format(
            &format!("{id}.format"),
            &format!("Check {id} formatting"),
            &["fmt", "--manifest-path", &manifest, "--check"],
            &format!("{id} source → formatter"),
        ),
        cargo(
            &format!("{id}.tests"),
            &format!("Test {id}"),
            &["test", "--manifest-path", &manifest],
            &format!("{id} source → tests"),
        ),
    ] {
        operation.inputs = vec![root.into()];
        out.entry(operation.id.clone()).or_insert(operation);
    }
    if depth == Depth::Verify {
        let mut operation = cargo(
            &format!("{id}.clippy"),
            &format!("Lint {id}"),
            &[
                "clippy",
                "--manifest-path",
                &manifest,
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            &format!("{id} source → clippy"),
        );
        operation.inputs = vec![root.into()];
        out.entry(operation.id.clone()).or_insert(operation);
    }
}

fn cargo(id: &str, title: &str, args: &[&str], reason: &str) -> Operation {
    let mut operation = op(id, title, "cargo", args, reason);
    operation.action = ActionKind::Cargo {
        offline_first: true,
    };
    operation
}
fn diff_check() -> Operation {
    Operation {
        id: "repo.diff-check".into(),
        title: "Check patch whitespace".into(),
        risk: Risk::ReadOnly,
        action: ActionKind::Git,
        phase: WorkflowPhase::Cheap,
        program: "git".into(),
        args: vec!["diff".into(), "--check".into()],
        reason: "all patches require whitespace validation".into(),
        failure_hint: "inspect with scripts/agent run show RUN_ID".into(),
        inputs: Vec::new(),
    }
}
fn cargo_format(id: &str, title: &str, args: &[&str], reason: &str) -> Operation {
    let mut operation = op(id, title, "cargo", args, reason);
    operation.action = ActionKind::Cargo {
        offline_first: false,
    };
    operation
}
fn op(id: &str, title: &str, program: &str, args: &[&str], reason: &str) -> Operation {
    Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
        action: ActionKind::Script,
        phase: WorkflowPhase::Host,
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        reason: reason.into(),
        failure_hint: "inspect with scripts/agent run show RUN_ID".into(),
        inputs: Vec::new(),
    }
}
fn op_owned(id: &str, title: &str, program: &str, args: Vec<String>, reason: &str) -> Operation {
    Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
        action: ActionKind::Script,
        phase: WorkflowPhase::Cheap,
        program: program.into(),
        args,
        reason: reason.into(),
        failure_hint: "inspect with scripts/agent run show RUN_ID".into(),
        inputs: Vec::new(),
    }
}
fn with_inputs(mut operation: Operation, inputs: &[&str]) -> Operation {
    operation.inputs = inputs.iter().map(|input| (*input).into()).collect();
    operation
}
fn apple_container(mut operation: Operation) -> Operation {
    operation.risk = Risk::LocalWrite;
    operation.action = ActionKind::AppleContainer;
    operation.phase = WorkflowPhase::Expensive;
    operation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;
    use std::process::Command;

    #[test]
    fn catalog_plan_selects_builder_and_reader_without_duplicates() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "crates/catalog/src/catalog_build_record.rs".into(),
                "crates/catalog/src/catalog_build_record.rs".into(),
            ],
        )
        .unwrap();
        let ids: Vec<_> = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "catalog.builder-tests",
                "catalog.format",
                "catalog.reader-check"
            ]
        );
        assert!(plan.operations[0].args.contains(&"builder".into()));
    }

    #[test]
    fn launcher_session_plan_uses_binary_ui_tests() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/ui_runner/launcher_catalog_session.rs".into()],
        )
        .unwrap();
        let tests = plan
            .operations
            .iter()
            .find(|operation| operation.id == "app.ui-tests")
            .unwrap();
        assert!(!tests.args.contains(&"--lib".into()));
        assert!(tests
            .args
            .windows(2)
            .any(|args| args == ["--bin", "mister-magik-fb"]));
        assert!(tests.args.contains(&"ui".into()));
        assert!(tests
            .args
            .contains(&"launcher_catalog_session::tests".into()));
    }

    #[test]
    fn check_is_narrower_than_verify() {
        let paths = vec!["crates/catalog/src/lib.rs".into()];
        let check = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            paths.clone(),
        )
        .unwrap();
        let verify = affected_plan(
            Intent::Verify {
                scope: Scope::Paths(vec![]),
            },
            paths,
        )
        .unwrap();
        assert!(check.operations.len() < verify.operations.len());
    }

    #[test]
    fn cargo_dependency_policy_is_not_embedded_in_plans() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["agent-cli/src/executor.rs".into()],
        )
        .unwrap();
        for operation in &plan.operations {
            assert!(!operation.args.contains(&"--offline".into()));
            assert!(!operation.args.contains(&"--locked".into()));
        }
        let format = plan
            .operations
            .iter()
            .find(|operation| operation.id == "agent-cli.format")
            .unwrap();
        assert_eq!(format.args.first().map(String::as_str), Some("fmt"));
    }

    #[test]
    fn semantic_tooling_changes_select_doctor_contract() {
        for path in [
            ".codex/config.toml",
            ".lspi/config.toml",
            "scripts/rust-analyzer",
            "apps/mister/rust-toolchain.toml",
        ] {
            let plan = affected_plan(
                Intent::Check {
                    scope: Scope::Paths(vec![]),
                },
                vec![path.into()],
            )
            .unwrap();
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id == "host.doctor-tests"),
                "missing doctor contract for {path}"
            );
        }
    }

    #[test]
    fn rust_analyzer_wrapper_selects_shell_syntax_check() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["scripts/rust-analyzer".into()],
        )
        .unwrap();
        assert!(plan
            .operations
            .iter()
            .any(|operation| { operation.id == "script.syntax.scripts-rust-analyzer" }));
    }

    #[test]
    fn deleted_script_does_not_select_a_syntax_check() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["scripts/definitely-deleted-script.sh".into()],
        )
        .unwrap();
        assert!(plan
            .operations
            .iter()
            .all(|operation| !operation.id.starts_with("script.syntax.")));
        assert!(plan
            .operations
            .iter()
            .any(|operation| operation.id == "scripts.licenses"));
    }

    #[test]
    fn fpga_verification_requires_external_build_and_local_checks() {
        let plan = affected_plan(
            Intent::Verify {
                scope: crate::model::Scope::Paths(Vec::new()),
            },
            vec!["mister/platform/fpga/menu-vblank-latch/menu.sv".into()],
        )
        .unwrap();
        assert_eq!(plan.external_requirements.len(), 1);
        assert!(!plan.operations.is_empty());
        assert!(plan
            .operations
            .iter()
            .all(|operation| !operation.program.to_ascii_lowercase().contains("quartus")));
    }

    #[test]
    fn fpga_change_requires_external_rbf_build() {
        let plan = affected_plan(
            Intent::Verify {
                scope: Scope::Paths(vec![]),
            },
            vec!["mister/platform/fpga/menu-vblank-latch/menu.sv".into()],
        )
        .unwrap();
        assert_eq!(plan.external_requirements.len(), 1);
        assert!(plan.external_requirements[0]
            .message
            .contains("Under no circumstances"));
    }

    #[test]
    fn unclassified_path_fails_closed() {
        let error = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["new-subsystem/source.xyz".into()],
        )
        .unwrap_err();
        assert!(error.contains("unclassified task paths"));
    }

    #[test]
    fn launcher_verify_selects_canonical_local_arm_check() {
        let plan = affected_plan(
            Intent::Verify {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/ui/launcher.slint".into()],
        )
        .unwrap();
        let arm = plan
            .operations
            .iter()
            .find(|operation| operation.id == "arm.check-launcher")
            .unwrap();
        assert_eq!(arm.program, "scripts/agent");
        assert_eq!(arm.args, ["build", "validate-launcher"]);
        assert_eq!(arm.risk, Risk::LocalWrite);
    }

    #[test]
    fn every_tracked_repository_path_is_classified() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let output = Command::new("git")
            .args(["ls-files", "-z"])
            .current_dir(repository)
            .output()
            .unwrap();
        assert!(output.status.success());
        let tracked: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned()))
            .collect();
        let unclassified: Vec<_> = tracked
            .iter()
            .filter(|path| !classified(path))
            .cloned()
            .collect();
        assert!(unclassified.is_empty(), "unclassified: {unclassified:?}");
        let unmapped: Vec<_> = tracked
            .into_iter()
            .filter(|path| {
                let plan = affected_plan(
                    Intent::Check {
                        scope: Scope::Paths(vec![]),
                    },
                    vec![path.clone()],
                )
                .unwrap();
                plan.operations.is_empty() && plan.external_requirements.is_empty()
            })
            .collect();
        assert!(unmapped.is_empty(), "mapped to no validation: {unmapped:?}");
    }
}
