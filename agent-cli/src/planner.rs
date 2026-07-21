// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{ExternalRequirement, Intent, Operation, Plan, Risk};
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
    for operation in &mut operations {
        if operation.inputs.is_empty() {
            operation.inputs = inferred_inputs(operation);
        }
    }
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

fn inferred_inputs(operation: &Operation) -> Vec<String> {
    let root = if operation.id.starts_with("app.") || operation.id.starts_with("arm.") {
        Some("apps/mister")
    } else if operation.id.starts_with("desktop.") {
        Some("apps/desktop")
    } else if operation.id.starts_with("documentation.") {
        Some("documentation")
    } else if operation.id.starts_with("kernel.") {
        Some("mister/platform/kernel")
    } else if operation.id.starts_with("fpga.") {
        Some("mister/platform/fpga")
    } else if operation.id.starts_with("scripts.") || operation.id.starts_with("script.syntax.") {
        Some("scripts")
    } else if operation.id.starts_with("tools.") {
        Some("tools")
    } else {
        None
    };
    root.into_iter().map(str::to_owned).collect()
}

#[must_use]
pub fn workflow_plan(intent: Intent) -> Plan {
    let operations = match &intent {
        Intent::Doctor => vec![op(
            "doctor.full-host",
            "Inspect host prerequisites",
            "python3",
            &["scripts/lib/doctor.py", "--scope", "full-host"],
            "host environment requested",
        )],
        _ => Vec::new(),
    };
    Plan {
        intent,
        operations,
        external_requirements: Vec::new(),
    }
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
    let mut add = |operation: Operation| {
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
        add(crate::registry::operation("repo.diff-check").unwrap());
    }
    if path.starts_with("agent-cli") {
        add(with_inputs(
            cargo(
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
            cargo(
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
        add(crate::registry::operation("repo.diff-check").unwrap());
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
            if path.starts_with("apps/mister/ui") || path.starts_with("apps/mister/src/ui_runner") {
                add(local_write(op(
                    "arm.check-launcher",
                    "Check launcher in Apple container",
                    "scripts/agent",
                    &["build", "validate-launcher"],
                    "launcher source → ARM validation",
                )));
            } else {
                add(local_write(op(
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
        let text = path.to_string_lossy();
        if text.contains("platform-bundle") || text.contains("quartus") {
            add(op(
                "scripts.quartus-cache",
                "Test fake Quartus cache contract",
                "scripts/tests/test-quartus-r2-cache.sh",
                &[],
                "Quartus cache workflow changed",
            ));
        }
    }
    if path.starts_with("docs") && !path.starts_with("docs/agents") {
        add(crate::registry::operation("repo.diff-check").unwrap());
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
    if path == Path::new("scripts/mister") || text.contains("mister-magik-agent") {
        add(op(
            "scripts.mister-safety",
            "Check MiSTer wrapper safety",
            "scripts/checks/check-no-main-kill.sh",
            &[],
            "MiSTer wrapper changed",
        ));
        add(op(
            "scripts.mister-guidance",
            "Check MiSTer wrapper guidance",
            "python3",
            &["scripts/checks/check-agent-guidance.py"],
            "MiSTer wrapper changed",
        ));
    }
    if text.contains("quartus-r2-cache") || text.contains("install-quartus-lite") {
        add(op(
            "scripts.quartus-cache",
            "Test fake Quartus cache contract",
            "scripts/tests/test-quartus-r2-cache.sh",
            &[],
            "Quartus cache tooling changed",
        ));
    }
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
    if text.contains("bench") || text.contains("profile-catalog") {
        add(op(
            "scripts.catalog-gates",
            "Test catalog benchmark gates",
            "python3",
            &["scripts/checks/check-catalog-contention.py", "--self-test"],
            "benchmark tooling changed",
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

#[allow(dead_code)]
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
    .expect("representative paths are classified")
    .operations
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn release_operations() -> Vec<Operation> {
    let mut operations = full_host_operations();
    operations.push(local_write(op(
        "arm.build-device",
        "Build release device binary",
        "scripts/agent",
        &["build", "runtime-device"],
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
        inputs: Vec::new(),
    }
}
fn op_owned(id: &str, title: &str, program: &str, args: Vec<String>, reason: &str) -> Operation {
    Operation {
        id: id.into(),
        title: title.into(),
        risk: Risk::ReadOnly,
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
#[allow(dead_code)]
fn local_write(mut operation: Operation) -> Operation {
    operation.risk = Risk::LocalWrite;
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
    fn mister_wrapper_does_not_select_quartus() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["scripts/mister".into()],
        )
        .unwrap();
        assert!(plan
            .operations
            .iter()
            .all(|operation| !operation.id.contains("quartus")));
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
    fn quartus_cache_test_is_owned_by_quartus_tooling() {
        let plan = affected_plan(
            Intent::Check {
                scope: Scope::Paths(vec![]),
            },
            vec!["scripts/quartus-r2-cache.sh".into()],
        )
        .unwrap();
        assert!(plan
            .operations
            .iter()
            .any(|operation| operation.id == "scripts.quartus-cache"));
        assert!(plan.external_requirements.is_empty());
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
