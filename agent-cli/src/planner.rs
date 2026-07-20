// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{ArmTask, Intent, Operation, Plan, Risk, RustTask};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Depth {
    Check,
    Verify,
}

#[must_use]
pub fn affected_plan(intent: Intent, paths: Vec<PathBuf>) -> Plan {
    let depth = if matches!(intent, Intent::Verify { .. }) {
        Depth::Verify
    } else {
        Depth::Check
    };
    let paths: BTreeSet<_> = paths.into_iter().collect();
    let mut operations = BTreeMap::new();
    for path in &paths {
        add_path_operations(path, depth, &mut operations);
    }
    Plan {
        intent,
        operations: operations.into_values().collect(),
    }
}

#[must_use]
pub fn workflow_plan(intent: Intent) -> Plan {
    let operations = match &intent {
        Intent::VerifyFullHost => full_host_operations(),
        Intent::Doctor => vec![op(
            "doctor.full-host",
            "Inspect host prerequisites",
            "python3",
            &["scripts/lib/doctor.py", "--scope", "full-host"],
            "host environment requested",
        )],
        Intent::Rust { task } => match task {
            RustTask::Format => vec![cargo(
                "app.format",
                "Check MiSTer app formatting",
                &[
                    "fmt",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--check",
                ],
                "Rust format requested",
            )],
            RustTask::Test => vec![cargo(
                "app.tests",
                "Test MiSTer host logic",
                &[
                    "test",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--lib",
                    "--no-default-features",
                ],
                "Rust tests requested",
            )],
            RustTask::Check => vec![cargo(
                "app.check",
                "Check MiSTer host logic",
                &[
                    "check",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--lib",
                    "--no-default-features",
                ],
                "Rust check requested",
            )],
        },
        Intent::HostTools { full } => host_tool_operations(*full),
        Intent::ReleaseHost => release_operations(),
        Intent::Arm { task } => vec![match task {
            ArmTask::CheckLib => local_write(op(
                "arm.check-lib",
                "Check library in Apple container",
                "apps/mister/build-arm.sh",
                &["--check", "--lib-only"],
                "ARM library confidence required",
            )),
            ArmTask::CheckLauncher => local_write(op(
                "arm.check-launcher",
                "Check launcher in Apple container",
                "apps/mister/build-arm.sh",
                &["--check", "--ui-scope", "launcher"],
                "ARM launcher confidence required",
            )),
            ArmTask::CheckArcade => local_write(op(
                "arm.check-arcade",
                "Check arcade UI in Apple container",
                "apps/mister/build-arm.sh",
                &["--check", "--ui-scope", "arcade"],
                "ARM arcade confidence required",
            )),
            ArmTask::CheckAll => local_write(op(
                "arm.check-all",
                "Check all UI in Apple container",
                "apps/mister/build-arm.sh",
                &["--check", "--ui-scope", "all", "--experiments"],
                "complete ARM UI confidence required",
            )),
            ArmTask::BuildDevice => local_write(op(
                "arm.build-device",
                "Build device binary in Apple container",
                "apps/mister/build-arm.sh",
                &["--device"],
                "device binary requested",
            )),
        }],
        _ => Vec::new(),
    };
    Plan { intent, operations }
}

fn add_path_operations(path: &Path, depth: Depth, out: &mut BTreeMap<String, Operation>) {
    let mut add = |operation: Operation| {
        out.entry(operation.id.clone()).or_insert(operation);
    };
    if path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md") {
        for operation in host_tool_operations(false) {
            add(operation);
        }
        return;
    }
    if path.starts_with("agent-cli") {
        add(cargo(
            "agent-cli.format",
            "Check agent-cli formatting",
            &["fmt", "--manifest-path", "agent-cli/Cargo.toml", "--check"],
            "agent-cli source → formatter",
        ));
        add(cargo(
            "agent-cli.tests",
            "Test agent-cli",
            &["test", "--manifest-path", "agent-cli/Cargo.toml"],
            "agent-cli source → unit tests",
        ));
        if depth == Depth::Verify {
            add(cargo(
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
            ));
        }
    }
    if path.starts_with("crates/catalog") {
        add(cargo(
            "catalog.format",
            "Check catalog formatting",
            &[
                "fmt",
                "--manifest-path",
                "crates/catalog/Cargo.toml",
                "--check",
            ],
            "catalog source → formatter",
        ));
        add(cargo(
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
        ));
        add(cargo(
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
        ));
        if depth == Depth::Verify {
            add(cargo(
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
            ));
        }
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("AGENTS.md")
        && (path.starts_with("apps/mister/src")
            || path.starts_with("apps/mister/ui")
            || matches!(
                path.to_str(),
                Some(
                    "apps/mister/Cargo.toml"
                        | "apps/mister/Cargo.lock"
                        | "apps/mister/build.rs"
                        | "apps/mister/rust-toolchain.toml"
                )
            ))
    {
        add(cargo(
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
        }
    }
    if path.starts_with("scripts")
        || path.starts_with(".github")
        || path.starts_with(".githooks")
        || path.ends_with("AGENTS.md")
        || path.starts_with("docs/agents")
    {
        for operation in host_tool_operations(false) {
            add(operation);
        }
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
    add_crate(path, "mister/tools/host", "mister-host", depth, out);
    add_crate(path, "mister/tools/agent", "mister-agent", depth, out);
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
    for operation in [
        cargo(
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
        out.entry(operation.id.clone()).or_insert(operation);
    }
    if depth == Depth::Verify {
        let operation = cargo(
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
        out.entry(operation.id.clone()).or_insert(operation);
    }
}

fn full_host_operations() -> Vec<Operation> {
    let representative = [
        "agent-cli/src/main.rs",
        "crates/catalog/src/lib.rs",
        "crates/magik-core/src/lib.rs",
        "mister/platform/runtime/src/lib.rs",
        "apps/mister/src/lib.rs",
        "mister/tools/host/src/main.rs",
        "mister/tools/agent/src/main.rs",
        "crates/framebuffer-stream/src/lib.rs",
        "scripts/agent",
        "documentation/src/content/docs/index.mdx",
    ];
    let intent = Intent::Verify {
        scope: crate::model::Scope::Paths(Vec::new()),
    };
    affected_plan(
        intent,
        representative.into_iter().map(PathBuf::from).collect(),
    )
    .operations
}

fn host_tool_operations(full: bool) -> Vec<Operation> {
    let mut operations = vec![
        op(
            "host.licenses",
            "Check license headers",
            "python3",
            &["scripts/checks/check-license-headers.py"],
            "tooling change → license contract",
        ),
        op(
            "host.guidance",
            "Check agent guidance",
            "python3",
            &["scripts/checks/check-agent-guidance.py"],
            "tooling change → guidance contract",
        ),
        op(
            "host.layout",
            "Check repository layout",
            "python3",
            &["scripts/checks/check-repository-layout.py"],
            "tooling change → layout contract",
        ),
        op(
            "host.validation-tests",
            "Test typed validation routing",
            "cargo",
            &[
                "test",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "planner::tests",
            ],
            "tooling change → routing tests",
        ),
        op(
            "host.catalog-contention",
            "Test catalog contention gate",
            "python3",
            &["scripts/checks/check-catalog-contention.py", "--self-test"],
            "tooling change → benchmark gate",
        ),
        op(
            "host.catalog-rebuild",
            "Test catalog rebuild gate",
            "python3",
            &["scripts/checks/check-catalog-rebuild.py", "--self-test"],
            "tooling change → benchmark gate",
        ),
        op(
            "host.doctor-tests",
            "Test host doctor",
            "python3",
            &["scripts/tests/test-doctor.py"],
            "tooling change → doctor contract",
        ),
        op(
            "host.no-main-kill",
            "Check Main process safety",
            "scripts/checks/check-no-main-kill.sh",
            &[],
            "tooling change → process safety",
        ),
        op(
            "host.no-direct-arcade",
            "Check arcade launch safety",
            "scripts/checks/check-no-direct-arcade-scene.sh",
            &[],
            "tooling change → launch safety",
        ),
        op(
            "host.diagnostics",
            "Check compact diagnostics",
            "scripts/checks/check-compact-diagnostic-output.sh",
            &[],
            "tooling change → diagnostic contract",
        ),
        op(
            "host.scanout",
            "Check scanout contract",
            "scripts/checks/check-scanout-slots-contract.sh",
            &[],
            "tooling change → scanout contract",
        ),
        op(
            "host.kernel-workflow",
            "Test kernel scanout workflow",
            "python3",
            &["scripts/tests/test-kernel-scanout-workflows.py"],
            "tooling change → CI contract",
        ),
        op(
            "host.platform-workflow",
            "Test platform workflow",
            "python3",
            &["scripts/tests/test-platform-bundle-workflow.py"],
            "tooling change → platform contract",
        ),
        op(
            "host.platform-selection",
            "Test platform artifact selection",
            "python3",
            &["scripts/tests/test-platform-artifact-selection.py"],
            "tooling change → artifact contract",
        ),
        op(
            "host.release-selection",
            "Test published release selection",
            "python3",
            &["scripts/tests/test-select-published-release.py"],
            "tooling change → release contract",
        ),
        op(
            "host.database-workflow",
            "Test game database workflow",
            "python3",
            &["scripts/tests/test-game-databases-workflow.py"],
            "tooling change → database contract",
        ),
        op(
            "host.distribution",
            "Test distribution workflow",
            "python3",
            &["scripts/tests/test-distribution-workflow.py"],
            "tooling change → distribution contract",
        ),
        op(
            "host.arm-contract",
            "Test ARM build contract",
            "python3",
            &["scripts/tests/test-arm-build-contract.py"],
            "tooling change → ARM contract",
        ),
        op(
            "host.cache-identity",
            "Test cache identity",
            "python3",
            &["scripts/tests/test-ci-cache-identity.py"],
            "tooling change → cache contract",
        ),
        op(
            "host.cache-contract",
            "Test CI cache contract",
            "python3",
            &["scripts/tests/test-ci-cache-contract.py"],
            "tooling change → CI cache contract",
        ),
        op(
            "host.quartus-cache",
            "Test Quartus cache",
            "scripts/tests/test-quartus-r2-cache.sh",
            &[],
            "tooling change → Quartus cache",
        ),
        op(
            "host.apple-resources",
            "Test Apple container resources",
            "scripts/tests/test-apple-container-resources.sh",
            &[],
            "tooling change → Apple container contract",
        ),
    ];
    if full {
        operations.extend([
            op(
                "host.magik-mode",
                "Test MagiK mode switching",
                "scripts/tests/test-magik-mode.sh",
                &[],
                "full host verification → mode contract",
            ),
            op(
                "host.installer",
                "Test MiSTer MagiK installer",
                "scripts/tests/test-mister-magik-installer.sh",
                &[],
                "full host verification → installer contract",
            ),
            op(
                "host.platform-id",
                "Test platform component identity",
                "python3",
                &["scripts/tests/test-platform-component-id.py"],
                "full host verification → component identity",
            ),
            op(
                "host.platform-bundle",
                "Test platform bundle",
                "python3",
                &["scripts/tests/test-platform-bundle.py"],
                "full host verification → platform bundle",
            ),
            op(
                "host.embedded-catalog",
                "Test embedded catalog release",
                "python3",
                &["scripts/tests/test-embedded-catalog-release.py"],
                "full host verification → catalog release",
            ),
            op(
                "host.database-bundle",
                "Test game database bundle",
                "python3",
                &["scripts/tests/test-game-databases-bundle.py"],
                "full host verification → database bundle",
            ),
            op(
                "host.downloader-db",
                "Test downloader database generation",
                "python3",
                &["scripts/tests/test-generate-downloader-db.py"],
                "full host verification → downloader database",
            ),
            cargo(
                "host.mister-tests",
                "Test MiSTer host tool",
                &["test", "--manifest-path", "mister/tools/host/Cargo.toml"],
                "full host verification → host tool tests",
            ),
        ]);
    }
    operations
}

fn release_operations() -> Vec<Operation> {
    let mut operations = full_host_operations();
    operations.push(local_write(op(
        "arm.build-device",
        "Build release device binary",
        "apps/mister/build-arm.sh",
        &["--device"],
        "release gate → ARM binary",
    )));
    operations.push(op(
        "arm.shared-libs",
        "Check ARM shared libraries",
        "apps/mister/scripts/check-arm-shared-libs.sh",
        &["apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"],
        "release gate → shared library contract",
    ));
    operations
}

fn cargo(id: &str, title: &str, args: &[&str], reason: &str) -> Operation {
    op(id, title, "cargo", args, reason)
}
fn op(id: &str, title: &str, program: &str, args: &[&str], reason: &str) -> Operation {
    Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
        program: program.into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        reason: reason.into(),
        failure_hint: "inspect with scripts/agent run show RUN_ID".into(),
    }
}
fn local_write(mut operation: Operation) -> Operation {
    operation.risk = Risk::LocalWrite;
    operation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;

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
        );
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
        );
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
        );
        let verify = affected_plan(
            Intent::Verify {
                scope: Scope::Paths(vec![]),
            },
            paths,
        );
        assert!(check.operations.len() < verify.operations.len());
    }

    #[test]
    fn cargo_dependency_policy_is_not_embedded_in_plans() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["agent-cli/src/executor.rs".into()],
        );
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
    fn apple_container_launcher_check_uses_canonical_command() {
        let plan = workflow_plan(Intent::Arm {
            task: ArmTask::CheckLauncher,
        });
        assert_eq!(plan.operations[0].program, "apps/mister/build-arm.sh");
        assert_eq!(
            plan.operations[0].args,
            ["--check", "--ui-scope", "launcher"]
        );
    }
}
