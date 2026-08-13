// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{
    ActionKind, AssuranceRequest, BuiltinOperation, ExternalRequirement, Operation, Plan, Risk,
    WorkflowPhase,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MISTER_APP_COMPILED_INPUTS: &[&str] = &[
    "apps/mister",
    "crates/catalog",
    "crates/framebuffer-stream",
    "crates/magik-core",
    "crates/media-contract",
    "crates/particles",
    "crates/screenshot-parade",
    "mister/platform/contracts/latch",
    "mister/platform/contracts/scanout",
    "mister/platform/contracts/video-diagnostics",
    "mister/platform/runtime",
];

pub fn affected_plan(request: AssuranceRequest, paths: Vec<PathBuf>) -> Result<Plan, String> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("agent-cli must live below the repository root");
    affected_plan_at(repository, request, paths)
}

pub fn affected_plan_at(
    repository: &Path,
    request: AssuranceRequest,
    paths: Vec<PathBuf>,
) -> Result<Plan, String> {
    let paths: BTreeSet<_> = paths.into_iter().collect();
    let unclassified: Vec<_> = paths
        .iter()
        .filter(|path| !classified(path))
        .map(|path| path.display().to_string())
        .collect();
    if !unclassified.is_empty() {
        return Err(format!(
            "unclassified changed paths: {}; add them to the typed impact map",
            unclassified.join(", ")
        ));
    }
    let mut operations = BTreeMap::new();
    let mut operation_conflicts = Vec::new();
    let mut external_requirements = Vec::new();
    for path in &paths {
        add_path_operations(repository, path, &mut operations, &mut operation_conflicts);
        if path.starts_with("mister/platform/fpga") {
            external_requirements.push(rbf_external_requirement());
        }
    }
    if !operation_conflicts.is_empty() {
        operation_conflicts.sort();
        operation_conflicts.dedup();
        return Err(format!(
            "conflicting operation definitions: {}",
            operation_conflicts.join(", ")
        ));
    }
    external_requirements.sort_by(|left, right| left.id.cmp(&right.id));
    external_requirements.dedup_by(|left, right| left.id == right.id);
    let mut operations: Vec<_> = operations.into_values().collect();
    operations.sort_by(|left, right| {
        left.workflow_phase()
            .cmp(&right.workflow_phase())
            .then_with(|| left.id.cmp(&right.id))
    });
    let operations = combine_arm_validation(subsume_cargo(normalize_operations(operations)));
    Ok(Plan {
        request,
        operations,
        external_requirements,
    })
}

fn combine_arm_validation(mut operations: Vec<Operation>) -> Vec<Operation> {
    let launcher = operations
        .iter()
        .position(|operation| operation.id == "arm.check-launcher");
    let library = operations
        .iter()
        .position(|operation| operation.id == "arm.check-lib");
    if let (Some(launcher), Some(library)) = (launcher, library) {
        let mut inputs = operations[launcher].inputs.clone();
        inputs.extend(operations[library].inputs.clone());
        inputs.sort();
        inputs.dedup();
        let first = launcher.min(library);
        let second = launcher.max(library);
        operations.remove(second);
        operations.remove(first);
        let mut combined = apple_container(op(
            "arm.check-runtime",
            "Check launcher and library in shared Apple container setup",
            "scripts/agent",
            &["build", "validate-runtime"],
            "mixed runtime changes → combined ARM validation",
        ));
        combined.inputs = inputs;
        operations.push(combined);
        operations.sort_by(|left, right| {
            left.workflow_phase()
                .cmp(&right.workflow_phase())
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    operations
}

fn normalize_operations(operations: Vec<Operation>) -> Vec<Operation> {
    let mut normalized: Vec<Operation> = Vec::new();
    for mut operation in operations {
        if let Some(existing) = normalized.iter_mut().find(|existing| {
            existing.program == operation.program
                && existing.args == operation.args
                && existing.action == operation.action
                && existing.risk == operation.risk
                && existing.workflow_phase() == operation.workflow_phase()
        }) {
            existing.inputs.append(&mut operation.inputs);
            existing.inputs.sort();
            existing.inputs.dedup();
            if !existing.reason.contains(&operation.reason) {
                existing.reason.push_str("; ");
                existing.reason.push_str(&operation.reason);
            }
        } else {
            normalized.push(operation);
        }
    }
    normalized
}

fn subsume_cargo(mut operations: Vec<Operation>) -> Vec<Operation> {
    let covered: BTreeSet<_> = operations
        .iter()
        .enumerate()
        .filter(|(_, operation)| {
            operation.program == "cargo"
                && operation.args.first().is_some_and(|arg| arg == "check")
                && operation.args.iter().any(|arg| arg == "--all-targets")
        })
        .filter_map(|(index, check)| {
            operations
                .iter()
                .any(|clippy| cargo_clippy_subsumes(clippy, check))
                .then_some(index)
        })
        .collect();
    operations = operations
        .into_iter()
        .enumerate()
        .filter_map(|(index, operation)| (!covered.contains(&index)).then_some(operation))
        .collect();
    operations
}

fn cargo_clippy_subsumes(clippy: &Operation, check: &Operation) -> bool {
    if clippy.program != "cargo"
        || clippy.args.first().is_none_or(|arg| arg != "clippy")
        || !clippy.args.iter().any(|arg| arg == "--all-targets")
    {
        return false;
    }
    let signature = |operation: &Operation| {
        operation
            .args
            .iter()
            .skip(1)
            .take_while(|arg| arg.as_str() != "--")
            .filter(|arg| arg.as_str() != "--all-targets")
            .cloned()
            .collect::<Vec<_>>()
    };
    signature(clippy) == signature(check)
}

fn classified(path: &Path) -> bool {
    crate::components::classify(path).is_some()
}

fn is_root_file(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
}

fn is_repository_dot_config(path: &Path) -> bool {
    crate::components::is_repository_dot_config(path)
}

fn rbf_external_requirement() -> ExternalRequirement {
    ExternalRequirement {
        id: "fpga.rbf-signoff".into(),
        message: "FPGA signoff required: on Apple Silicon, use only the typed `scripts/agent fpga setup` and `scripts/agent fpga signoff` workflow. GitHub release qualification remains owned by the `Build MiSTer MagiK Platform` workflow. Never invoke Quartus, its installer, or the FPGA build script directly.".into(),
    }
}

fn add_path_operations(
    repository: &Path,
    path: &Path,
    out: &mut BTreeMap<String, Operation>,
    conflicts: &mut Vec<String>,
) {
    let default_input = path.display().to_string();
    let mut add = |mut operation: Operation| {
        if operation.inputs.is_empty() {
            operation.inputs.push(default_input.clone());
        }
        merge_operation(out, operation, conflicts);
    };
    if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
        || path.starts_with("scripts")
        || path.starts_with("apps/mister/scripts")
        || path.starts_with(".github/workflows")
    {
        add(builtin(
            "repo.shell-ownership",
            "Check shell and Main FIFO ownership",
            BuiltinOperation::ShellOwnership,
            "maintained Rust or shell source changed → command ownership contract",
        ));
    }
    if path.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md")
        || path.starts_with("docs/agents")
    {
        add(builtin(
            "repo.guidance",
            "Check agent guidance",
            BuiltinOperation::AgentGuidance,
            "agent guidance changed",
        ));
    }
    if path.starts_with(".codex") || path == Path::new("apps/mister/rust-toolchain.toml") {
        add(cargo(
            "host.doctor-tests",
            "Test host doctor",
            &[
                "test",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "doctor::tests",
            ],
            "host tooling changed → doctor contract",
        ));
    }
    if is_root_file(path)
        || is_repository_dot_config(path)
        || path.starts_with("LICENSES")
        || path.starts_with("history")
        || path.starts_with("private")
    {
        add(diff_check());
    }
    if crate::components::classify(path) == Some(crate::components::Component::AgentCli) {
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
        {
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
        add(with_inputs(
            cargo(
                "agent-cli.binary",
                "Build runnable agent CLI",
                &[
                    "build",
                    "--manifest-path",
                    "agent-cli/Cargo.toml",
                    "--bin",
                    "agent-cli",
                ],
                "agent-cli source → runnable operator binary",
            ),
            &["crates/agent-protocol", "agent-cli"],
        ));
        add(with_inputs(
            cargo(
                "agent-cli.signed-media-tests",
                "Test signed-manifest agent CLI mode",
                &[
                    "test",
                    "--manifest-path",
                    "agent-cli/Cargo.toml",
                    "--no-default-features",
                    "--features",
                    "signed-media-manifests",
                ],
                "agent-cli media source → signed-manifest feature tests",
            ),
            &[
                "crates/agent-protocol",
                "crates/media-contract",
                "agent-cli",
            ],
        ));
    }
    if path.starts_with("crates/agent-protocol") {
        add(with_inputs(
            cargo(
                "protocol.host-binary",
                "Build host protocol consumer",
                &[
                    "build",
                    "--manifest-path",
                    "agent-cli/Cargo.toml",
                    "--bin",
                    "agent-cli",
                ],
                "agent protocol → runnable host consumer",
            ),
            &["crates/agent-protocol", "agent-cli"],
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
                ],
                "catalog source → reader check",
            ),
            &["crates/catalog"],
        ));
        {
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
    if path.starts_with("apps/framebuffer-lab") {
        add(diff_check());
        add(with_inputs(
            cargo_format(
                "framebuffer-lab.format",
                "Check framebuffer lab formatting",
                &[
                    "fmt",
                    "--manifest-path",
                    "apps/framebuffer-lab/Cargo.toml",
                    "--check",
                ],
                "framebuffer lab source → formatter",
            ),
            &["apps/framebuffer-lab"],
        ));
        add(with_inputs(
            cargo(
                "framebuffer-lab.tests",
                "Test framebuffer lab",
                &[
                    "test",
                    "--manifest-path",
                    "apps/framebuffer-lab/Cargo.toml",
                    "--all-targets",
                ],
                "framebuffer lab source → tests",
            ),
            &["apps/framebuffer-lab"],
        ));
        add(with_inputs(
            cargo(
                "framebuffer-lab.clippy",
                "Lint framebuffer lab",
                &[
                    "clippy",
                    "--manifest-path",
                    "apps/framebuffer-lab/Cargo.toml",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
                "framebuffer lab source → clippy",
            ),
            &["apps/framebuffer-lab"],
        ));
    }
    if path.starts_with("apps/framebuffer-scene-lab")
        || path.starts_with("apps/startup-particle-lab")
    {
        add(diff_check());
    }
    if matches!(
        path.to_str(),
        Some(
            "apps/mister/Cargo.toml"
                | "apps/mister/src/media_http.rs"
                | "apps/mister/src/media_bench_download.rs"
                | "apps/mister/src/ui_runner/media_worker.rs"
        )
    ) {
        for (id, title, features) in [
            (
                "app.media-http-default-tests",
                "Test unsigned-manifest app mode",
                "ui",
            ),
            (
                "app.media-http-signed-tests",
                "Test signed-manifest app mode",
                "ui,signed-media-manifests",
            ),
        ] {
            add(with_inputs(
                cargo(
                    id,
                    title,
                    &[
                        "test",
                        "--manifest-path",
                        "apps/mister/Cargo.toml",
                        "--bin",
                        "mister-magik-fb",
                        "--no-default-features",
                        "--features",
                        features,
                        "media_http::tests",
                    ],
                    "app media source → manifest feature tests",
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        }
    }
    if path == Path::new("apps/mister/Cargo.toml")
        || path == Path::new("apps/mister/src/bin/ui_preview.rs")
        || path == Path::new("apps/mister/src/preview_state.rs")
        || path == Path::new("apps/mister/src/visual_composition.rs")
        || path == Path::new("apps/mister/src/ui_runner/launcher_screensaver.rs")
        || path.starts_with("crates/screenshot-parade")
        || path.starts_with("apps/mister/ui")
        || path.starts_with("apps/mister/ui-generated")
    {
        add(with_inputs(
            cargo(
                "app.preview-ui-tests",
                "Test preview UI feature matrix",
                &[
                    "test",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--lib",
                    "--no-default-features",
                    "--features",
                    "ui-preview",
                ],
                "preview UI source → complete preview UI suite",
            ),
            MISTER_APP_COMPILED_INPUTS,
        ));
        add(with_inputs(
            cargo(
                "app.ui-preview-tests",
                "Test macOS UI preview",
                &[
                    "test",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--bin",
                    "mister-magik-ui-preview",
                    "--no-default-features",
                    "--features",
                    "ui-preview",
                ],
                "preview or Slint source → display-profile and scenario tests",
            ),
            MISTER_APP_COMPILED_INPUTS,
        ));
        {
            add(with_inputs(
                cargo(
                    "app.ui-preview-binary",
                    "Build macOS UI preview",
                    &[
                        "build",
                        "--manifest-path",
                        "apps/mister/Cargo.toml",
                        "--bin",
                        "mister-magik-ui-preview",
                        "--no-default-features",
                        "--features",
                        "ui-preview",
                    ],
                    "preview or Slint source → headless visual capture binary",
                ),
                MISTER_APP_COMPILED_INPUTS,
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
        add(with_inputs(
            cargo_format(
                "app.format",
                "Check MiSTer app formatting",
                &[
                    "fmt",
                    "--manifest-path",
                    "apps/mister/Cargo.toml",
                    "--check",
                ],
                "MiSTer app source → formatter",
            ),
            &["apps/mister"],
        ));
        if path == Path::new("apps/mister/Cargo.toml")
            || path == Path::new("apps/mister/src/ui_display.rs")
            || path == Path::new("apps/mister/src/arcade_list_renderer.rs")
            || path.starts_with("apps/mister/src/ui_runner")
        {
            add(with_inputs(
                cargo(
                    "app.production-ui-tests",
                    "Test production UI feature matrix",
                    &[
                        "test",
                        "--manifest-path",
                        "apps/mister/Cargo.toml",
                        "--bin",
                        "mister-magik-fb",
                        "--no-default-features",
                        "--features",
                        "ui",
                    ],
                    "production UI source → complete production UI suite",
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        }
        if path == Path::new("apps/mister/Cargo.toml")
            || path.starts_with("apps/mister/src/experiments")
        {
            add(with_inputs(
                cargo(
                    "app.experimental-ui-tests",
                    "Test experimental UI feature matrix",
                    &[
                        "test",
                        "--manifest-path",
                        "apps/mister/Cargo.toml",
                        "--lib",
                        "--no-default-features",
                        "--features",
                        "ui,experiments",
                    ],
                    "experimental UI source → complete experimental UI suite",
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        }
        if path == Path::new("apps/mister/Cargo.toml")
            || path == Path::new("apps/mister/src/command_args.rs")
            || path == Path::new("apps/mister/src/visual_platform.rs")
            || path.starts_with("apps/mister/src/ui_runner")
            || path.starts_with("apps/mister/ui-generated")
        {
            add(with_inputs(
                cargo(
                    "app.bench-scenes-tests",
                    "Test bench-scenes UI feature matrix",
                    &[
                        "test",
                        "--manifest-path",
                        "apps/mister/Cargo.toml",
                        "--lib",
                        "--no-default-features",
                        "--features",
                        "ui,bench-scenes",
                    ],
                    "benchmark scene source → complete bench-scenes UI suite",
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        }
        if path.starts_with("apps/mister/src/ui_runner") {
            add(with_inputs(
                cargo(
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
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        } else {
            add(with_inputs(
                cargo(
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
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
        }
        {
            add(with_inputs(
                cargo(
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
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
            add(with_inputs(
                cargo(
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
                ),
                MISTER_APP_COMPILED_INPUTS,
            ));
            if path.starts_with("apps/mister/ui") || path.starts_with("apps/mister/src/ui_runner") {
                add(with_inputs(
                    apple_container(op(
                        "arm.check-launcher",
                        "Check launcher in Apple container",
                        "scripts/agent",
                        &["build", "validate-launcher"],
                        "launcher source → ARM validation",
                    )),
                    MISTER_APP_COMPILED_INPUTS,
                ));
            } else {
                add(with_inputs(
                    apple_container(op(
                        "arm.check-lib",
                        "Check library in Apple container",
                        "scripts/agent",
                        &["build", "validate-library"],
                        "MiSTer source → ARM validation",
                    )),
                    MISTER_APP_COMPILED_INPUTS,
                ));
            }
        }
    }
    if path.starts_with("mister/platform/kernel") {
        add(builtin(
            "kernel.workflow-contract",
            "Test kernel scanout workflow",
            BuiltinOperation::KernelWorkflow,
            "kernel source → workflow contract",
        ));
    }
    if path.starts_with("mister/platform/fpga") {
        add(builtin(
            "fpga.workflow-contract",
            "Test platform workflow",
            BuiltinOperation::PlatformWorkflow,
            "FPGA source → workflow contract",
        ));
    }
    if path.starts_with("tools") {
        add(diff_check());
    }
    if path.starts_with("scripts") {
        if repository.join(path).exists() {
            add_script_operations(repository, path, &mut add);
        } else {
            add(diff_check());
        }
    }
    if matches!(
        path.to_str(),
        Some(
            ".githooks/pre-commit"
                | "scripts/checks/pre-commit.py"
                | "scripts/checks/run-with-deadline.py"
                | "scripts/tests/test-pre-commit.py"
        )
    ) {
        add(with_inputs(
            op(
                "scripts.pre-commit-contract",
                "Test the pre-commit contract",
                "python3",
                &["scripts/tests/test-pre-commit.py"],
                "pre-commit tooling changed",
            ),
            &[
                ".githooks/pre-commit",
                "scripts/checks/pre-commit.py",
                "scripts/checks/run-with-deadline.py",
                "scripts/tests/test-pre-commit.py",
            ],
        ));
    }
    if path.starts_with("apps/mister/ui")
        || matches!(
            path.to_str(),
            Some(
                "scripts/checks/check-font-text-contract.py"
                    | "scripts/checks/pre-commit.py"
                    | "scripts/tests/test-font-text-contract.py"
            )
        )
    {
        add(with_inputs(
            op(
                "scripts.font-text-contract",
                "Check font text contract",
                "python3",
                &[
                    "scripts/checks/check-font-text-contract.py",
                    "--repository",
                    ".",
                    "--all",
                ],
                "first-party Slint text changed",
            ),
            &[
                "apps/mister/ui",
                "scripts/checks/check-font-text-contract.py",
                "scripts/tests/test-font-text-contract.py",
            ],
        ));
    }
    if matches!(
        path.to_str(),
        Some(
            "scripts/checks/check-font-text-contract.py"
                | "scripts/tests/test-font-text-contract.py"
        )
    ) {
        add(with_inputs(
            op(
                "scripts.font-text-contract-tests",
                "Test font text contract",
                "python3",
                &["scripts/tests/test-font-text-contract.py"],
                "font text contract tooling changed",
            ),
            &[
                "scripts/checks/check-font-text-contract.py",
                "scripts/tests/test-font-text-contract.py",
            ],
        ));
    }
    if path.starts_with("apps/mister/ui")
        || path.starts_with("apps/mister/ui-generated")
        || path == Path::new("scripts/tests/test-slint-build-contract.py")
    {
        let mut contract = with_inputs(
            op(
                "scripts.slint-build-contract",
                "Test Slint build dependency tracking",
                "python3",
                &["scripts/tests/test-slint-build-contract.py"],
                "Slint UI or build glue changed",
            ),
            &[
                "apps/mister/ui",
                "apps/mister/ui-generated",
                "scripts/tests/test-slint-build-contract.py",
            ],
        );
        contract.phase = WorkflowPhase::Expensive;
        add(contract);
    }
    if path.starts_with(".github") || path.starts_with(".githooks") {
        add(builtin(
            "repo.workflow-contract",
            "Check workflow contracts",
            BuiltinOperation::CiCache,
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
        {
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
    if path.starts_with("crates/media-contract") {
        add(with_inputs(
            cargo(
                "media-contract.signed-tests",
                "Test signed-manifest media contract",
                &[
                    "test",
                    "--manifest-path",
                    "crates/media-contract/Cargo.toml",
                    "--no-default-features",
                    "--features",
                    "signed-media-manifests",
                ],
                "media contract → signed-manifest feature tests",
            ),
            &["crates/media-contract"],
        ));
    }
    add_crate(path, "crates/magik-core", "magik-core", out);
    add_crate(path, "crates/framebuffer-scenes", "framebuffer-scenes", out);
    add_crate(path, "crates/particles", "particles", out);
    add_crate(path, "crates/perf-events", "perf-events", out);
    add_crate(path, "crates/screenshot-parade", "screenshot-parade", out);
    if path.starts_with("crates/screenshot-parade") {
        add_crate(
            Path::new("apps/framebuffer-scene-lab"),
            "apps/framebuffer-scene-lab",
            "framebuffer-scene-lab",
            out,
        );
        for id in [
            "framebuffer-scene-lab.format",
            "framebuffer-scene-lab.tests",
            "framebuffer-scene-lab.clippy",
        ] {
            if let Some(operation) = out.get_mut(id) {
                operation.inputs.push("crates/screenshot-parade".into());
            }
        }
        let mut operation = diff_check();
        operation.inputs.push(default_input.clone());
        merge_operation(out, operation, conflicts);
    }
    if path.starts_with("apps/startup-particle-lab") {
        add_crate(
            Path::new("apps/framebuffer-scene-lab"),
            "apps/framebuffer-scene-lab",
            "framebuffer-scene-lab",
            out,
        );
    } else {
        add_crate(
            path,
            "apps/framebuffer-scene-lab",
            "framebuffer-scene-lab",
            out,
        );
    }
    add_crate(path, "crates/framebuffer-stream", "framebuffer-stream", out);
    add_crate(path, "crates/agent-protocol", "agent-protocol", out);
    add_crate(path, "crates/media-contract", "media-contract", out);
    add_crate(path, "crates/mister-ini", "mister-ini", out);
    add_crate(path, "mister/platform/runtime", "mister-runtime", out);
    add_crate(
        path,
        "mister/platform/contracts/latch",
        "latch-contract",
        out,
    );
    add_crate(
        path,
        "mister/platform/contracts/scanout",
        "scanout-contract",
        out,
    );
    add_crate(
        path,
        "mister/platform/contracts/video-diagnostics",
        "video-diagnostics-contract",
        out,
    );
    add_crate(path, "mister/tools/agent", "mister-agent", out);
    add_crate(path, "mister/tools/manager", "mister-manager", out);
}

fn merge_operation(
    out: &mut BTreeMap<String, Operation>,
    mut operation: Operation,
    conflicts: &mut Vec<String>,
) {
    match out.entry(operation.id.clone()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(operation);
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let existing = entry.get_mut();
            if existing.title != operation.title
                || existing.risk != operation.risk
                || existing.action != operation.action
                || existing.phase != operation.phase
                || existing.program != operation.program
                || existing.args != operation.args
                || existing.failure_hint != operation.failure_hint
                || existing.builtin != operation.builtin
            {
                conflicts.push(operation.id);
                return;
            }
            existing.inputs.append(&mut operation.inputs);
            existing.inputs.sort();
            existing.inputs.dedup();
            if !existing.reason.contains(&operation.reason) {
                existing.reason.push_str("; ");
                existing.reason.push_str(&operation.reason);
            }
        }
    }
}

fn add_script_operations(repository: &Path, path: &Path, add: &mut impl FnMut(Operation)) {
    let text = path.to_string_lossy();
    add(builtin(
        "scripts.licenses",
        "Check script license headers",
        BuiltinOperation::LicenseHeaders,
        "script source → license contract",
    ));
    if repository.join(path).exists()
        && (path.extension().and_then(|extension| extension.to_str()) == Some("sh")
            || matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("agent" | "mister")
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
    if text.contains("platform-bundle") {
        add(builtin(
            "scripts.platform-workflow",
            "Test platform workflow",
            BuiltinOperation::PlatformWorkflow,
            "platform tooling changed",
        ));
    }
    if text.contains("install") || text.contains("distribution") || text.contains("package-") {
        add(builtin(
            "scripts.distribution",
            "Test distribution workflow",
            BuiltinOperation::DistributionWorkflow,
            "packaging tooling changed",
        ));
    }
    if text.ends_with("MiSTer-MagiK.sh") || text.ends_with("test-mister-magik-installer.sh") {
        add(with_inputs(
            cargo(
                "mister-manager.host-binary",
                "Build host installer manager fixture",
                &[
                    "build",
                    "--manifest-path",
                    "mister/tools/manager/Cargo.toml",
                    "--bin",
                    "mister-magik-manager",
                ],
                "installer lifecycle fixture requires the host manager binary",
            ),
            &["mister/tools/manager"],
        ));
        let mut lifecycle = with_inputs(
            op_owned(
                "scripts.installer-lifecycle",
                "Test installer lifecycle",
                "bash",
                vec!["scripts/tests/test-mister-magik-installer.sh".into()],
                "installer or its lifecycle fixture changed",
            ),
            &[
                "scripts/MiSTer-MagiK.sh",
                "scripts/tests/test-mister-magik-installer.sh",
            ],
        );
        lifecycle.phase = WorkflowPhase::Expensive;
        add(lifecycle);
    }
}

fn add_crate(path: &Path, root: &str, id: &str, out: &mut BTreeMap<String, Operation>) {
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
    {
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
        builtin: None,
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
        builtin: None,
    }
}
fn builtin(id: &str, title: &str, operation: BuiltinOperation, reason: &str) -> Operation {
    let mut planned = op(id, title, "agent-cli", &[], reason);
    planned.action = ActionKind::Builtin;
    planned.builtin = Some(operation);
    planned
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
        builtin: None,
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
    fn framebuffer_lab_changes_select_only_standalone_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/framebuffer-lab/src/main.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "repo.diff-check",
                "framebuffer-lab.clippy",
                "framebuffer-lab.format",
                "framebuffer-lab.tests"
            ]
        );
        assert!(plan.operations.iter().all(|operation| {
            !operation
                .args
                .iter()
                .any(|argument| argument.contains("apps/mister/Cargo.toml"))
        }));
    }

    #[test]
    fn framebuffer_scene_lab_changes_select_only_focused_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/framebuffer-scene-lab/src/main.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "repo.diff-check",
                "framebuffer-scene-lab.clippy",
                "framebuffer-scene-lab.format",
                "framebuffer-scene-lab.tests"
            ]
        );
        assert!(plan.operations.iter().all(|operation| {
            !operation
                .args
                .iter()
                .any(|argument| argument.contains("apps/mister/Cargo.toml"))
        }));
    }

    #[test]
    fn retired_particle_lab_paths_select_the_canonical_scene_lab_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/startup-particle-lab/src/main.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "repo.diff-check",
                "framebuffer-scene-lab.clippy",
                "framebuffer-scene-lab.format",
                "framebuffer-scene-lab.tests"
            ]
        );
        assert!(plan.operations.iter().all(|operation| {
            !operation
                .args
                .iter()
                .any(|argument| argument.contains("apps/startup-particle-lab"))
        }));
    }

    #[test]
    fn particle_engine_changes_select_only_shared_crate_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["crates/particles/src/engine.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["particles.clippy", "particles.format", "particles.tests"]
        );
        assert!(plan.operations.iter().all(|operation| {
            !operation
                .args
                .iter()
                .any(|argument| argument.contains("apps/mister/Cargo.toml"))
        }));
    }

    #[test]
    fn perf_event_changes_select_focused_crate_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["crates/perf-events/src/lib.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "perf-events.clippy",
                "perf-events.format",
                "perf-events.tests"
            ]
        );
    }

    #[test]
    fn framebuffer_scene_changes_select_shared_crate_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["crates/framebuffer-scenes/src/navigation.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "framebuffer-scenes.clippy",
                "framebuffer-scenes.format",
                "framebuffer-scenes.tests"
            ]
        );
        assert!(plan.operations.iter().all(|operation| {
            !operation
                .args
                .iter()
                .any(|argument| argument.contains("apps/mister/Cargo.toml"))
        }));
    }

    #[test]
    fn screenshot_parade_changes_select_crate_lab_and_application_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["crates/screenshot-parade/src/schedule.rs".into()],
        )
        .unwrap();
        let ids = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<BTreeSet<_>>();
        for expected in [
            "screenshot-parade.clippy",
            "screenshot-parade.format",
            "screenshot-parade.tests",
            "framebuffer-scene-lab.clippy",
            "framebuffer-scene-lab.format",
            "framebuffer-scene-lab.tests",
            "app.preview-ui-tests",
            "app.ui-preview-binary",
            "app.ui-preview-tests",
        ] {
            assert!(ids.contains(expected), "missing {expected}");
        }
        assert!(plan.operations.iter().all(|operation| {
            !operation.id.starts_with("framebuffer-scene-lab.")
                || operation
                    .inputs
                    .contains(&"crates/screenshot-parade".into())
        }));
    }

    #[test]
    fn catalog_plan_selects_builder_and_reader_without_duplicates() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
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
                "catalog.clippy",
                "catalog.format",
                "catalog.reader-check"
            ]
        );
        assert!(plan.operations[0].args.contains(&"builder".into()));
    }

    #[test]
    fn pre_commit_contract_changes_select_python_fixtures_once() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                ".githooks/pre-commit".into(),
                "scripts/checks/pre-commit.py".into(),
                "scripts/checks/run-with-deadline.py".into(),
                "scripts/tests/test-pre-commit.py".into(),
            ],
        )
        .unwrap();
        let operations: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.id == "scripts.pre-commit-contract")
            .collect();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].args, ["scripts/tests/test-pre-commit.py"]);
    }

    #[test]
    fn video_diagnostics_contract_changes_select_crate_assurance() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["mister/platform/contracts/video-diagnostics/src/lib.rs".into()],
        )
        .unwrap();
        let ids: BTreeSet<_> = plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str())
            .collect();
        for id in [
            "video-diagnostics-contract.format",
            "video-diagnostics-contract.tests",
            "video-diagnostics-contract.clippy",
        ] {
            assert!(ids.contains(id), "missing {id}");
        }
    }

    #[test]
    fn slint_changes_select_the_full_font_text_contract_once() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "apps/mister/ui/launcher.slint".into(),
                "apps/mister/ui/components/start2p.slint".into(),
                "scripts/checks/check-font-text-contract.py".into(),
            ],
        )
        .unwrap();
        let operations: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.id == "scripts.font-text-contract")
            .collect();
        assert_eq!(operations.len(), 1);
        assert_eq!(
            operations[0].args,
            [
                "scripts/checks/check-font-text-contract.py",
                "--repository",
                ".",
                "--all"
            ]
        );
        let test_operations: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.id == "scripts.font-text-contract-tests")
            .collect();
        assert_eq!(test_operations.len(), 1);
        assert_eq!(
            test_operations[0].args,
            ["scripts/tests/test-font-text-contract.py"]
        );
    }

    #[test]
    fn imported_slint_changes_select_compilation_and_dependency_contract_once() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "apps/mister/ui/components/combo_box.slint".into(),
                "apps/mister/ui-generated/build.rs".into(),
                "scripts/tests/test-slint-build-contract.py".into(),
            ],
        )
        .unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|operation| operation.id == "app.ui-check")
        );
        let operations: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.id == "scripts.slint-build-contract")
            .collect();
        assert_eq!(operations.len(), 1);
        assert_eq!(
            operations[0].args,
            ["scripts/tests/test-slint-build-contract.py"]
        );
        assert_eq!(operations[0].phase, WorkflowPhase::Expensive);
    }

    #[test]
    fn launcher_session_plan_uses_binary_ui_tests() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
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
        assert!(
            tests
                .args
                .windows(2)
                .any(|args| args == ["--bin", "mister-magik-fb"])
        );
        assert!(tests.args.contains(&"ui".into()));
        assert!(
            tests
                .args
                .contains(&"launcher_catalog_session::tests".into())
        );
    }

    #[test]
    fn ui_assurance_selects_explicit_supported_feature_matrices() {
        let production = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/ui_display.rs".into()],
        )
        .unwrap();
        let production_test = production
            .operations
            .iter()
            .find(|operation| operation.id == "app.production-ui-tests")
            .unwrap();
        assert!(production_test.args.contains(&"ui".into()));
        assert!(
            production_test
                .args
                .windows(2)
                .any(|args| args == ["--bin", "mister-magik-fb"])
        );
        assert!(!production_test.args.contains(&"ui,experiments".into()));

        let preview = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/preview_state.rs".into()],
        )
        .unwrap();
        let preview_test = preview
            .operations
            .iter()
            .find(|operation| operation.id == "app.preview-ui-tests")
            .unwrap();
        assert!(preview_test.args.contains(&"--lib".into()));
        assert!(preview_test.args.contains(&"ui-preview".into()));

        let bench = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/command_args.rs".into()],
        )
        .unwrap();
        let bench_test = bench
            .operations
            .iter()
            .find(|operation| operation.id == "app.bench-scenes-tests")
            .unwrap();
        assert!(bench_test.args.contains(&"ui,bench-scenes".into()));

        let experimental = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/experiments/particles/showcase.rs".into()],
        )
        .unwrap();
        let experimental_test = experimental
            .operations
            .iter()
            .find(|operation| operation.id == "app.experimental-ui-tests")
            .unwrap();
        assert!(experimental_test.args.contains(&"ui,experiments".into()));
    }

    #[test]
    fn ui_preview_changes_select_profile_tests_and_capture_binary() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/bin/ui_preview.rs".into()],
        )
        .unwrap();
        for id in ["app.ui-preview-tests", "app.ui-preview-binary"] {
            let operation = plan
                .operations
                .iter()
                .find(|operation| operation.id == id)
                .unwrap_or_else(|| panic!("missing {id}"));
            assert!(operation.args.contains(&"ui-preview".into()));
            assert_eq!(operation.inputs, MISTER_APP_COMPILED_INPUTS);
        }
    }

    #[test]
    fn app_format_cache_covers_the_complete_formatted_crate() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["apps/mister/src/ui_runner/crt_trial_loop.rs".into()],
        )
        .unwrap();
        let format = plan
            .operations
            .iter()
            .find(|operation| operation.id == "app.format")
            .unwrap();

        assert_eq!(format.inputs, ["apps/mister"]);
    }

    #[test]
    fn plan_pre_push_and_ci_assurance_share_the_full_plan() {
        let paths = vec!["crates/catalog/src/lib.rs".into()];
        let planned = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(paths.clone()),
            },
            paths.clone(),
        )
        .unwrap();
        let pre_push = affected_plan(
            AssuranceRequest::PrePush {
                remote: "origin".into(),
            },
            paths.clone(),
        )
        .unwrap();
        let ci = affected_plan(
            AssuranceRequest::CiHostAssurance {
                scope: Scope::Paths(paths.clone()),
            },
            paths,
        )
        .unwrap();

        assert_eq!(planned.operations, pre_push.operations);
        assert_eq!(planned.operations, ci.operations);
        assert_eq!(
            planned.external_requirements,
            pre_push.external_requirements
        );
        assert_eq!(planned.external_requirements, ci.external_requirements);
    }

    #[test]
    fn ordinary_assurance_never_selects_an_optimized_cargo_profile() {
        let paths = vec![
            "agent-cli/src/lib.rs".into(),
            "apps/mister/Cargo.toml".into(),
            "apps/framebuffer-lab/src/lib.rs".into(),
            "crates/catalog/src/lib.rs".into(),
            "mister/tools/agent/src/main.rs".into(),
        ];
        let plan = affected_plan(
            AssuranceRequest::PrePush {
                remote: "origin".into(),
            },
            paths,
        )
        .unwrap();
        for operation in plan
            .operations
            .iter()
            .filter(|operation| matches!(operation.action, ActionKind::Cargo { .. }))
        {
            assert!(!operation.args.iter().any(|arg| arg == "--release"));
            assert!(!operation.args.iter().any(|arg| arg == "--profile"));
        }
    }

    #[test]
    fn protocol_and_host_changes_refresh_the_runnable_agent_cli_binary() {
        for path in [
            "crates/agent-protocol/src/lib.rs",
            "agent-cli/src/host/agent_client.rs",
        ] {
            let plan = affected_plan(
                AssuranceRequest::Plan {
                    scope: Scope::Paths(vec![]),
                },
                vec![path.into()],
            )
            .unwrap();
            assert!(plan.operations.iter().any(|operation| {
                operation.args.first().map(String::as_str) == Some("build")
                    && operation.args.contains(&"--bin".into())
                    && operation.args.contains(&"agent-cli".into())
            }));
        }
    }

    #[test]
    fn cargo_dependency_policy_is_not_embedded_in_plans() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
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
    fn host_tooling_changes_select_doctor_contract() {
        for path in [".codex/config.toml", "apps/mister/rust-toolchain.toml"] {
            let plan = affected_plan(
                AssuranceRequest::Plan {
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
    fn repository_dot_config_changes_select_diff_check() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![".obsolete/config.toml".into()],
        )
        .unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|operation| operation.id == "repo.diff-check")
        );
        assert!(
            plan.operations
                .iter()
                .all(|operation| operation.id != "repo.workflow-contract")
        );
    }

    #[test]
    fn workflow_dot_directories_keep_workflow_contract() {
        for path in [".github/workflows/check.yml", ".githooks/pre-commit"] {
            let plan = affected_plan(
                AssuranceRequest::Plan {
                    scope: Scope::Paths(vec![]),
                },
                vec![path.into()],
            )
            .unwrap();
            assert!(
                plan.operations
                    .iter()
                    .any(|operation| operation.id == "repo.workflow-contract")
            );
            assert!(
                plan.operations
                    .iter()
                    .all(|operation| operation.id != "repo.diff-check")
            );
        }
    }

    #[test]
    fn deleted_script_does_not_select_a_syntax_check() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["scripts/definitely-deleted-script.sh".into()],
        )
        .unwrap();
        assert!(
            plan.operations
                .iter()
                .all(|operation| !operation.id.starts_with("script.syntax."))
        );
        assert!(
            plan.operations
                .iter()
                .any(|operation| operation.id == "repo.diff-check")
        );
        assert!(
            plan.operations
                .iter()
                .all(|operation| operation.id != "scripts.licenses")
        );
    }

    #[test]
    fn fpga_verification_requires_typed_signoff_and_local_checks() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: crate::model::Scope::Paths(Vec::new()),
            },
            vec!["mister/platform/fpga/menu-vblank-latch/menu.sv".into()],
        )
        .unwrap();
        assert_eq!(plan.external_requirements.len(), 1);
        assert!(!plan.operations.is_empty());
        assert!(
            plan.operations
                .iter()
                .all(|operation| !operation.program.to_ascii_lowercase().contains("quartus"))
        );
    }

    #[test]
    fn fpga_change_routes_to_typed_local_or_github_signoff() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["mister/platform/fpga/menu-vblank-latch/menu.sv".into()],
        )
        .unwrap();
        assert_eq!(plan.external_requirements.len(), 1);
        assert!(
            plan.external_requirements[0]
                .message
                .contains("scripts/agent fpga signoff")
        );
        assert!(
            plan.external_requirements[0]
                .message
                .contains("Never invoke Quartus")
        );
    }

    #[test]
    fn unclassified_path_fails_closed() {
        let error = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec!["new-subsystem/source.xyz".into()],
        )
        .unwrap_err();
        assert!(error.contains("unclassified changed paths"));
    }

    #[test]
    fn launcher_verify_selects_canonical_local_arm_check() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
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
        assert_eq!(arm.inputs, MISTER_APP_COMPILED_INPUTS);
    }

    #[test]
    fn compiled_app_checks_fingerprint_all_local_dependency_roots() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "apps/mister/src/ui_runner/launcher_frame_accounting.rs".into(),
                "apps/mister/src/ui_runner/launcher_loop.rs".into(),
                "apps/mister/ui/components/overlays.slint".into(),
            ],
        )
        .unwrap();
        for id in [
            "app.clippy",
            "app.tests",
            "app.ui-check",
            "app.ui-tests",
            "arm.check-launcher",
        ] {
            let operation = plan
                .operations
                .iter()
                .find(|operation| operation.id == id)
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(operation.inputs, MISTER_APP_COMPILED_INPUTS, "{id}");
        }
    }

    #[test]
    fn media_manifest_changes_test_default_and_signed_feature_modes() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "apps/mister/src/media_http.rs".into(),
                "crates/media-contract/src/lib.rs".into(),
                "agent-cli/src/host/media.rs".into(),
            ],
        )
        .unwrap();

        for id in [
            "app.media-http-default-tests",
            "app.media-http-signed-tests",
            "media-contract.tests",
            "media-contract.signed-tests",
            "agent-cli.tests",
            "agent-cli.signed-media-tests",
        ] {
            assert!(
                plan.operations.iter().any(|operation| operation.id == id),
                "missing {id}"
            );
        }
        let signed_app = plan
            .operations
            .iter()
            .find(|operation| operation.id == "app.media-http-signed-tests")
            .unwrap();
        assert!(
            signed_app
                .args
                .iter()
                .any(|arg| arg == "ui,signed-media-manifests")
        );
    }

    #[test]
    fn mixed_runtime_changes_select_one_combined_arm_operation() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(Vec::new()),
            },
            vec![
                "apps/mister/ui/launcher.slint".into(),
                "apps/mister/src/media_update.rs".into(),
            ],
        )
        .unwrap();
        let arm: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.id.starts_with("arm.check"))
            .collect();
        assert_eq!(arm.len(), 1);
        assert_eq!(arm[0].id, "arm.check-runtime");
        assert_eq!(arm[0].args, ["build", "validate-runtime"]);
    }

    #[test]
    fn identical_executions_merge_reasons_and_inputs() {
        let mut first = cargo("first", "First", &["test"], "first reason");
        first.inputs = vec!["one".into()];
        let mut second = cargo("second", "Second", &["test"], "second reason");
        second.inputs = vec!["two".into()];
        let normalized = normalize_operations(vec![first, second]);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].inputs, ["one", "two"]);
        assert!(normalized[0].reason.contains("first reason"));
        assert!(normalized[0].reason.contains("second reason"));
    }

    #[test]
    fn duplicate_operation_ids_merge_inputs_and_reject_conflicts() {
        let mut operations = BTreeMap::new();
        let mut conflicts = Vec::new();
        let mut first = cargo("same", "Same", &["check"], "first reason");
        first.inputs = vec!["one".into()];
        merge_operation(&mut operations, first, &mut conflicts);
        let mut second = cargo("same", "Same", &["check"], "second reason");
        second.inputs = vec!["two".into(), "one".into()];
        merge_operation(&mut operations, second, &mut conflicts);
        assert!(conflicts.is_empty());
        assert_eq!(operations["same"].inputs, ["one", "two"]);
        assert!(operations["same"].reason.contains("first reason"));
        assert!(operations["same"].reason.contains("second reason"));

        let conflicting = cargo("same", "Same", &["test"], "conflicting reason");
        merge_operation(&mut operations, conflicting, &mut conflicts);
        assert_eq!(conflicts, ["same"]);
    }

    #[test]
    fn all_target_clippy_subsumes_matching_check_only() {
        let check = cargo(
            "check",
            "Check",
            &[
                "check",
                "--manifest-path",
                "crate/Cargo.toml",
                "--all-targets",
            ],
            "check",
        );
        let clippy = cargo(
            "clippy",
            "Clippy",
            &[
                "clippy",
                "--manifest-path",
                "crate/Cargo.toml",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            "clippy",
        );
        let operations = subsume_cargo(vec![check, clippy.clone()]);
        assert_eq!(operations, [clippy]);
    }

    #[test]
    fn maintained_rust_and_shell_changes_select_main_fifo_ownership_once() {
        let plan = affected_plan(
            AssuranceRequest::Plan {
                scope: Scope::Paths(vec![]),
            },
            vec![
                "apps/mister/src/new_module.rs".into(),
                "scripts/new-check.sh".into(),
            ],
        )
        .unwrap();
        let operations: Vec<_> = plan
            .operations
            .iter()
            .filter(|operation| operation.builtin == Some(BuiltinOperation::ShellOwnership))
            .collect();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].id, "repo.shell-ownership");
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
                    AssuranceRequest::Plan {
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
