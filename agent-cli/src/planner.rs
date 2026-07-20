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
            ArmTask::CheckLauncher => local_write(op(
                "arm.check-launcher",
                "Check launcher in Apple container",
                "apps/mister/build-arm.sh",
                &["--check", "--ui-scope", "launcher"],
                "ARM launcher confidence required",
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
            &[
                "test",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "--offline",
            ],
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
                    "--offline",
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
    if path.starts_with("apps/mister") {
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
                    "--no-default-features",
                    "--features",
                    "ui",
                ],
                "launcher session source → UI binary tests",
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
                "--offline",
                "planner::tests",
            ],
            "tooling change → routing tests",
        ),
    ];
    if full {
        operations.push(op(
            "host.all-script-tests",
            "Run full script contract tests",
            "python3",
            &["scripts/tests/test-kernel-scanout-workflows.py"],
            "full host verification requested",
        ));
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
        assert!(tests.args.contains(&"ui".into()));
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
