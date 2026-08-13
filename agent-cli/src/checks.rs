// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::BuiltinOperation;
use crate::progress::{EventKind, Reporter};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn execute(
    operation: BuiltinOperation,
    repository: &Path,
    reporter: &mut Reporter<'_>,
) -> Result<(), String> {
    reporter.emit(
        EventKind::Progress,
        "check",
        &format!("Checking {}", label(operation)),
        None,
    )?;
    run(operation, repository)
}

pub fn label(operation: BuiltinOperation) -> &'static str {
    match operation {
        BuiltinOperation::AgentGuidance => "agent guidance",
        BuiltinOperation::LicenseHeaders => "license headers",
        BuiltinOperation::ShellOwnership => "shell ownership",
        BuiltinOperation::RuntimeEnvironment => "runtime environment ownership",
        BuiltinOperation::DistributionWorkflow => "distribution workflow",
        BuiltinOperation::KernelWorkflow => "kernel workflow",
        BuiltinOperation::PlatformWorkflow => "platform workflow",
        BuiltinOperation::CiCache => "CI cache policy",
    }
}

pub fn run(operation: BuiltinOperation, repository: &Path) -> Result<(), String> {
    match operation {
        BuiltinOperation::AgentGuidance => check_agent_guidance(repository),
        BuiltinOperation::LicenseHeaders => check_license_headers(repository),
        BuiltinOperation::ShellOwnership => check_shell_ownership(repository),
        BuiltinOperation::RuntimeEnvironment => check_runtime_environment(repository),
        BuiltinOperation::DistributionWorkflow => check_distribution_workflow(repository),
        BuiltinOperation::KernelWorkflow => check_kernel_workflow(repository),
        BuiltinOperation::PlatformWorkflow => check_platform_workflow(repository),
        BuiltinOperation::CiCache => check_ci_cache(repository),
    }
}

fn require_fragments(
    label: &str,
    text: &str,
    required: &[&str],
    forbidden: &[&str],
) -> Result<(), String> {
    for fragment in required {
        if !text.contains(fragment) {
            return Err(format!("{label}_contract_missing: {fragment}"));
        }
    }
    for fragment in forbidden {
        if text.contains(fragment) {
            return Err(format!("{label}_contract_forbidden: {fragment}"));
        }
    }
    Ok(())
}

fn check_kernel_workflow(repository: &Path) -> Result<(), String> {
    let heavy = read(repository, ".github/workflows/kernel-scanout.yml")?;
    let light = read(repository, ".github/workflows/scanout-contract.yml")?;
    require_fragments(
        "kernel_heavy",
        &heavy,
        &[
            "contract-and-build:",
            "clang-build:",
            "coccinelle:",
            "Sparse type check",
            "Warning-clean rebuild",
            "mister/platform/kernel/scanout-slots/**",
        ],
        &[
            "workflow_dispatch:",
            "upload-artifact",
            "component_input_sha256=",
        ],
    )?;
    require_fragments(
        "kernel_light",
        &light,
        &[
            "scripts/checks/check-scanout-slots-contract.sh",
            "mister/platform/runtime/src/framebuffer/scanout_slots.rs",
            "mister/platform/contracts/scanout/src/lib.rs",
            "mister/tools/agent/src/scanout_slots_contract.rs",
            "agent-cli/src/platform_stage.rs",
            "agent-cli/src/host/platform_deploy.rs",
        ],
        &[
            "Linux-Kernel_MiSTer",
            "build-scanout-slots-module.sh",
            "coccinelle",
            "upload-artifact",
            "workflow_dispatch",
            "agent-cli/src/delivery.rs",
            "agent-cli/src/host/mod.rs",
        ],
    )
}

fn check_platform_workflow(repository: &Path) -> Result<(), String> {
    let workflow = read(repository, ".github/workflows/platform-bundle.yml")?;
    require_fragments(
        "platform_workflow",
        &workflow,
        &[
            "name: Build MiSTer MagiK Platform",
            "workflow_dispatch:",
            "Plan component reuse and builds",
            "scripts/agent ci platform-candidates",
            "scripts/agent ci platform-eligible-run",
            "scripts/agent ci platform-bundle plan-update",
            "[[ \"$run_id\" =~ ^[0-9]+$ ]]",
            "jq -er '.origin.run_id",
            "jq -er '.origin.head_sha",
            "resolve_equivalent fpga",
            "resolve_equivalent kernel",
            "fpga-synthesis-id:",
            "uses: actions/cache/restore@v6",
            "key: platform-fpga-synthesis-v0.1-",
            "uses: actions/cache/save@v6",
            "Reusing completed Quartus synthesis; all FPGA validation will run again.",
            "platform-bundle compact-component",
            "Download and verify latest platform release for assembly",
            "sha256sum -c -",
            "!build/fpga-signoff/**/Menu-work/**",
            "git diff --quiet \"$head_sha\" \"$GITHUB_SHA\"",
            "reused-from-latest-release",
            "reused-from-actions-cache",
            "platform-bundle-v0.2.json",
            "inputs.publish == true",
            "contents: write",
        ],
        &[
            "recover-platform-component.sh",
            "main-mister.yml",
            "fpga-vblank-latch.yml",
        ],
    )?;
    if workflow.matches("  workflow_dispatch:").count() != 1 {
        return Err("platform_workflow_contract: workflow_dispatch must occur once".into());
    }
    Ok(())
}

fn check_ci_cache(repository: &Path) -> Result<(), String> {
    let mut combined = String::new();
    let workflows = repository.join(".github/workflows");
    for entry in fs::read_dir(&workflows).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("yml") {
            combined.push_str(&fs::read_to_string(path).map_err(|error| error.to_string())?);
        }
    }
    for forbidden in [
        "actions/cache@v1",
        "actions/cache@v2",
        "actions/cache@v3",
        "actions/cache@v4",
        "actions/cache@v5",
        "ci-clippy",
        "target-clippy",
        "target-arm-dist",
        "cross-custom-rust",
    ] {
        if combined.contains(forbidden) {
            return Err(format!("ci_cache_contract_forbidden: {forbidden}"));
        }
    }
    let rust = read(repository, ".github/workflows/rust-arm.yml")?;
    require_fragments(
        "rust_arm_cache",
        &rust,
        &[
            "steps.cache-id.outputs.cargo_host",
            "steps.cache-id.outputs.cross_abi",
            "ci-cache-identity.py",
            "rustup default \"$toolchain\"",
            "scripts/agent ci host-assurance --paths ${{ matrix.paths }}",
            "scripts/agent build runtime-ci",
            "scripts/agent build device-agent-ci",
            "name: cargo-timings-ci-fast",
            "armv7-unknown-linux-gnueabihf/ci-fast",
            "if-no-files-found: error",
        ],
        &[
            "target-host-",
            "Cache host build outputs",
            "scripts/agent verify --paths desktop",
            "scripts/agent check",
            "runtime-fast",
            "release-device",
            "mister-magik-fb-release",
            "binary-size-release",
        ],
    )?;
    let distribution = read(repository, ".github/workflows/distribution.yml")?;
    require_fragments(
        "distribution_cache",
        &distribution,
        &[
            "uses: actions/cache/restore@v6",
            "target-arm-v2-",
            "packages: read",
            "GHCR_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
            "git rev-parse HEAD:private/magik-assets",
            "repository: NigelBreslaw/mister-magik-private-assets",
            "ref: ${{ steps.magik-assets-ref.outputs.sha }}",
            "path: private/magik-assets",
            "ssh-key: ${{ secrets.MAGIK_ASSETS_SSH_KEY }}",
        ],
        &[],
    )
}

fn check_distribution_workflow(repository: &Path) -> Result<(), String> {
    let workflow = read(repository, ".github/workflows/distribution.yml")?;
    let cross = read(repository, "apps/mister/Cross.toml")?;
    let package = read(repository, "scripts/package-distribution.sh")?;
    for variable in [
        "MISTER_MAGIK_BUILD_NUMBER",
        "MISTER_MAGIK_VERSION",
        "MISTER_MAGIK_BUILD_TIME",
        "MISTER_MAGIK_SOURCE_REVISION",
        "MISTER_MAGIK_SOURCE_DIRTY",
    ] {
        if !cross.contains(&format!("\"{variable}\"")) {
            return Err(format!("distribution_contract_missing: {variable}"));
        }
    }
    for required in [
        "release_channel:",
        "scripts/agent build runtime-device",
        "scripts/agent build manager-device",
        "armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb",
        "scripts/agent ci require-alpha-promotion",
        "scripts/agent ci platform-manifest generate",
        "scripts/agent ci game-databases verify",
        "contents: write",
        "gh release create",
        "--tag \"$RELEASE_TAG\"",
        "name: Publish ${{ github.event.inputs.release_channel }} release",
        "initialize_feed_branch()",
        "cancel-in-progress: false",
    ] {
        if !workflow.contains(required) {
            return Err(format!("distribution_contract_missing: {required}"));
        }
    }
    for forbidden in [
        "scripts/release/check-alpha-promotion.sh",
        "platform-manifest.py",
        "scripts/agent ci game-databases create",
        "mame-metadata-build",
        "--mame-sqlite",
        "--hbmame-sqlite",
    ] {
        if workflow.contains(forbidden) {
            return Err(format!("distribution_contract_forbidden: {forbidden}"));
        }
    }
    for forbidden in [
        "--mame-sqlite)",
        "--hbmame-sqlite)",
        "--hbmame-sqlite-default)",
        "--game-databases-manifest)",
    ] {
        if package.contains(forbidden) {
            return Err(format!("package_contract_forbidden: {forbidden}"));
        }
    }
    Ok(())
}

fn check_agent_guidance(repository: &Path) -> Result<(), String> {
    const REQUIRED: &[&str] = &[
        "docs/agents/README.md",
        "docs/agents/task-map.md",
        "docs/agents/file-authority.md",
        "apps/mister/AGENTS.md",
        "apps/mister/src/ui_runner/AGENTS.md",
        "mister/tools/agent/AGENTS.md",
        "scripts/AGENTS.md",
        "apps/desktop/AGENTS.md",
        "apps/mister/BUILD.md",
        "documentation/src/content/docs/contributing/workflow.mdx",
    ];
    let missing: Vec<_> = REQUIRED
        .iter()
        .filter(|path| !repository.join(path).is_file())
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(format!("agent_guidance_missing: {}", missing.join(", ")));
    }
    let authority = read(repository, "docs/agents/file-authority.md")?;
    for generator in [
        "scripts/media/harvest-core-launch-manifest.py",
        "scripts/release/packaging/generate-third-party-licenses.py",
    ] {
        if !authority.contains(generator) || !repository.join(generator).is_file() {
            return Err(format!("regeneration_command_missing: {generator}"));
        }
    }
    for path in [
        "build/agent-check.tmp",
        "outputs/agent-check.tmp",
        "target/agent-check.tmp",
        "documentation/node_modules/agent-check.tmp",
        "private/test-fixtures/agent-check.tmp",
        ".env.agent-check",
    ] {
        if !git_ignored(repository, path)? {
            return Err(format!("ignore_policy_missing: {path}"));
        }
    }
    let mut guidance = vec!["AGENTS.md"];
    guidance.extend_from_slice(REQUIRED);
    for path in guidance {
        let text = read(repository, path)?;
        for forbidden in [
            "scripts/agent check",
            "scripts/agent verify",
            "agent-linux-verify",
            "scripts/validate",
            "scripts/dev-rust",
            "scripts/doctor",
            "scripts/test-host-tools.sh",
            "scripts/release-check-host.sh",
            "cargo test",
            "cargo check",
            "cargo clippy",
            "cargo fmt",
            "apps/mister/build-arm.sh --check",
        ] {
            if text.contains(forbidden) {
                return Err(format!("workflow_bypass: {path} contains {forbidden}"));
            }
        }
    }
    let root = read(repository, "AGENTS.md")?;
    for expected in [
        "scripts/agent plan",
        "$magik-rust-lsp",
        "pre-push hook",
        "native Linux CI",
        "scripts/agent deliver",
        "git add --",
        "git commit -m",
        "first-attempt sandbox escalation",
    ] {
        if !root.contains(expected) {
            return Err(format!("root_workflow_missing: {expected}"));
        }
    }
    check_retired_validation_call_sites(repository)
}

fn check_retired_validation_call_sites(repository: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "grep",
            "-n",
            "-I",
            "-e",
            "scripts/agent check",
            "-e",
            "scripts/agent verify",
            "-e",
            "agent-linux-verify",
            "--",
            ".",
            ":(exclude)history/**",
            ":(exclude)reference/**",
            ":(exclude)agent-cli/src/checks.rs",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(1) => Ok(()),
        Some(0) => Err(format!(
            "retired_validation_interface: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

fn check_license_headers(repository: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot enumerate source files: {error}"))?;
    if !output.status.success() {
        return Err("cannot enumerate source files".into());
    }
    let mut failures = Vec::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = PathBuf::from(String::from_utf8_lossy(bytes).as_ref());
        if !license_target(&relative) || !repository.join(&relative).is_file() {
            continue;
        }
        let text = fs::read_to_string(repository.join(&relative))
            .map_err(|error| format!("cannot read {}: {error}", relative.display()))?;
        if !has_license_header(&relative, &text) {
            failures.push(relative.display().to_string());
        }
    }
    for manifest in [
        "apps/desktop/Cargo.toml",
        "crates/framebuffer-stream/Cargo.toml",
        "apps/mister/Cargo.toml",
        "crates/catalog/Cargo.toml",
        "apps/mister/ui-generated/Cargo.toml",
        "mister/tools/agent/Cargo.toml",
        "agent-cli/Cargo.toml",
    ] {
        if !read(repository, manifest)?.contains("license = \"GPL-3.0-or-later\"") {
            failures.push(format!("{manifest}: incorrect package license"));
        }
    }
    let package: serde_json::Value =
        serde_json::from_str(&read(repository, "documentation/package.json")?)
            .map_err(|error| format!("invalid documentation/package.json: {error}"))?;
    if package["license"] != "GPL-3.0-or-later" {
        failures.push("documentation/package.json: incorrect package license".into());
    }
    for path in ["COPYRIGHT", "LICENSES/GPL-3.0-or-later.txt", "REUSE.toml"] {
        if !repository.join(path).is_file() {
            failures.push(format!("{path}: missing licensing file"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("license_policy_failed: {}", failures.join(", ")))
    }
}

fn license_target(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if [
        "apps/desktop/vendor/",
        "documentation/public/screenshots/",
        "history/",
        "apps/mister/licenses/",
        "apps/mister/ui/art/",
        "apps/mister/ui/fonts/",
        "private/",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
    {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if matches!(
        name,
        "LICENSE"
            | "Cargo.lock"
            | "pnpm-lock.yaml"
            | "Menu_MiSTer-vblank-latched-fbuf.patch"
            | "Menu_MiSTer.commit"
    ) || name.ends_with(".lock")
    {
        return false;
    }
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default(),
        "astro"
            | "c"
            | "cpp"
            | "css"
            | "h"
            | "mjs"
            | "py"
            | "rs"
            | "sh"
            | "slint"
            | "sv"
            | "svg"
            | "swift"
            | "toml"
            | "ts"
            | "yaml"
            | "yml"
    ) || matches!(
        name,
        ".gitattributes" | ".gitignore" | ".gitmodules" | "Makefile"
    ) || name.starts_with("Dockerfile")
        || text == "scripts/platform-component-inputs/fpga-v0.1.txt"
        || text == "scripts/platform-component-inputs/kernel-v0.1.txt"
}

fn has_license_header(path: &Path, text: &str) -> bool {
    let first = text.lines().take(8).collect::<Vec<_>>().join("\n");
    let copyright = "Copyright (C) 2026 Nigel Breslaw";
    let license = "SPDX-License-Identifier: GPL-3.0-or-later";
    let Some(copyright_at) = first.find(copyright) else {
        return false;
    };
    let Some(license_at) = first.find(license) else {
        return false;
    };
    if path.starts_with("mister/platform/kernel/scanout-slots") {
        license_at < copyright_at
    } else {
        copyright_at < license_at
    }
}

fn check_shell_ownership(repository: &Path) -> Result<(), String> {
    const MAIN_FIFO_OWNERS: &[(&str, &str)] = &[
        (
            "apps/mister/src/launcher.rs",
            "temporary production app writer; migrate to mister/platform/runtime",
        ),
        (
            "crates/catalog/src/fs_fault.rs",
            "catalog destructive-fault capability",
        ),
        (
            "mister/tools/agent/src/main.rs",
            "device-service command capability",
        ),
        ("agent-cli/src/host/remote.rs", "host command construction"),
    ];
    const RETIRED: &[&str] = &[
        "scripts/magik-mode.sh",
        "scripts/run-rust.sh",
        "apps/mister/build-arm.sh",
        "apps/mister/build-arm64-apple-container.sh",
        "device-release-acceptance.sh",
        "device-startup-reveal-acceptance.sh",
        "device-launch-return-smoke.sh",
        "mister-video-mode-test.sh",
        "profile-first-scan.sh",
        "profile-first-preview.sh",
        "profile-preview-scroll.sh",
        "profile-arcade-scroll.sh",
        "bench-toolchain.sh",
        "deploy-rust.sh",
        "deploy-platform.sh",
    ];
    for path in &RETIRED[..5] {
        if repository.join(path).exists() {
            return Err(format!("retired_entrypoint_exists: {path}"));
        }
    }
    let files = repository_files(repository)?;
    let exclusions: BTreeSet<&str> = [
        "agent-cli/src/checks.rs",
        "docs/agents/script-deletion-ledger.md",
    ]
    .into_iter()
    .collect();
    for path in files {
        let relative = path.to_string_lossy();
        let maintained = relative == "AGENTS.md"
            || [
                "agent-cli/",
                "apps/",
                "docs/",
                "documentation/",
                "mister/",
                "scripts/",
                ".github/",
            ]
            .iter()
            .any(|prefix| relative.starts_with(prefix));
        if !maintained
            || exclusions.contains(relative.as_ref())
            || relative.starts_with("docs/performance-review-")
            || relative.starts_with("docs/2026-")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(repository.join(&path)) else {
            continue;
        };
        for retired in RETIRED {
            if text.contains(retired) {
                return Err(format!(
                    "retired_interface_reference: {relative} contains {retired}"
                ));
            }
        }
        if main_fifo_source(&path)
            && source_may_access_main_fifo(&text)
            && !MAIN_FIFO_OWNERS.iter().any(|(owner, _)| relative == *owner)
        {
            let owners = MAIN_FIFO_OWNERS
                .iter()
                .map(|(path, role)| format!("{path} ({role})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "main_fifo_writer_outside_owner: {relative}; production app command transport targets mister/platform/runtime; approved temporary owners: {owners}"
            ));
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct RuntimeEnvironmentRegistry {
    format: String,
    source_roots: Vec<String>,
    baseline: RuntimeEnvironmentBaseline,
    #[serde(default)]
    dynamic_prefix: Vec<RuntimeEnvironmentPrefix>,
    control: Vec<RuntimeEnvironmentControl>,
}

#[derive(serde::Deserialize)]
struct RuntimeEnvironmentBaseline {
    literal_occurrences: usize,
    unique_names: usize,
    external_build_names: usize,
}

#[derive(serde::Deserialize)]
struct RuntimeEnvironmentPrefix {
    prefix: String,
    owner: String,
    max_suffix_length: usize,
}

#[derive(Clone, serde::Deserialize)]
struct RuntimeEnvironmentControl {
    name: String,
    owner: String,
    classification: String,
    value_shape: String,
    default_behavior: String,
    visibility: String,
}

fn check_runtime_environment(repository: &Path) -> Result<(), String> {
    const REGISTRY_PATH: &str = "apps/mister/config/runtime-environment.toml";
    const REFERENCE_PATH: &str = "docs/reference/mister-runtime-environment.md";
    const FORMAT: &str = "mister-magik-runtime-environment-v1";
    const SOURCE_ROOTS: &[&str] = &[
        "apps/mister/src",
        "mister/platform/runtime/src",
        "crates/catalog/src",
        "crates/particles/src",
        "crates/perf-events/src",
    ];
    let text = read(repository, REGISTRY_PATH)?;
    let mut registry: RuntimeEnvironmentRegistry = toml::from_str(&text)
        .map_err(|error| format!("runtime_environment_registry_invalid: {error}"))?;
    if registry.format != FORMAT {
        return Err(format!(
            "runtime_environment_format_invalid: expected {FORMAT}"
        ));
    }
    if registry.source_roots != SOURCE_ROOTS {
        return Err("runtime_environment_source_roots_invalid".into());
    }
    let files = repository_files(repository)?;
    let mut actual = BTreeSet::new();
    let mut literal_occurrences = 0;
    for path in files.iter().filter(|path| {
        path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && SOURCE_ROOTS.iter().any(|root| path.starts_with(root))
    }) {
        let source = fs::read_to_string(repository.join(path))
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let names = mister_environment_names(&source);
        literal_occurrences += names.len();
        actual.extend(names);
    }
    let mut registered = BTreeSet::new();
    for control in &registry.control {
        if !valid_environment_name(&control.name)
            || !registered.insert(control.name.clone())
            || !SOURCE_ROOTS
                .iter()
                .any(|root| control.owner.starts_with(root))
            || !repository.join(&control.owner).is_file()
            || ![
                "production",
                "diagnostic",
                "benchmark",
                "preview",
                "test",
                "fault",
                "build-time",
                "deprecated",
                "external",
            ]
            .contains(&control.classification.as_str())
            || control.value_shape.is_empty()
            || control.default_behavior.is_empty()
            || control.visibility.is_empty()
        {
            return Err(format!(
                "runtime_environment_control_invalid: {}",
                control.name
            ));
        }
    }
    for prefix in &registry.dynamic_prefix {
        if !prefix.prefix.starts_with("MISTER_")
            || !prefix.prefix.ends_with('_')
            || prefix.max_suffix_length == 0
            || prefix.max_suffix_length > 64
            || !repository.join(&prefix.owner).is_file()
        {
            return Err(format!(
                "runtime_environment_prefix_invalid: {}",
                prefix.prefix
            ));
        }
    }
    let unregistered: Vec<_> = actual
        .difference(&registered)
        .filter(|name| !registered_by_prefix(name, &registry.dynamic_prefix))
        .cloned()
        .collect();
    if !unregistered.is_empty() {
        return Err(format!(
            "runtime_environment_unregistered: {}; register each control with its owning module in {REGISTRY_PATH}",
            unregistered.join(",")
        ));
    }
    let stale: Vec<_> = registered.difference(&actual).cloned().collect();
    if !stale.is_empty() {
        return Err(format!(
            "runtime_environment_stale_registry: {}",
            stale.join(",")
        ));
    }
    let external_build_names = registry
        .control
        .iter()
        .filter(|control| matches!(control.classification.as_str(), "external" | "build-time"))
        .count();
    if registry.baseline.literal_occurrences != literal_occurrences
        || registry.baseline.unique_names != actual.len()
        || registry.baseline.external_build_names != external_build_names
    {
        return Err(format!(
            "runtime_environment_baseline_drift: occurrences={literal_occurrences} names={} external_build={external_build_names}",
            actual.len()
        ));
    }
    registry
        .control
        .sort_by(|left, right| left.name.cmp(&right.name));
    let expected = render_runtime_environment_reference(&registry);
    let reference = read(repository, REFERENCE_PATH)?;
    if reference != expected {
        return Err(format!(
            "runtime_environment_reference_stale: regenerate {REFERENCE_PATH} from {REGISTRY_PATH}"
        ));
    }
    Ok(())
}

fn mister_environment_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names = Vec::new();
    let mut offset = 0;
    while offset + 7 < bytes.len() {
        if &bytes[offset..offset + 7] != b"MISTER_" {
            offset += 1;
            continue;
        }
        let start = offset;
        offset += 7;
        while offset < bytes.len()
            && (bytes[offset].is_ascii_uppercase()
                || bytes[offset].is_ascii_digit()
                || bytes[offset] == b'_')
        {
            offset += 1;
        }
        if offset > start + 7 {
            names.push(String::from_utf8_lossy(&bytes[start..offset]).into_owned());
        }
    }
    names
}

fn valid_environment_name(name: &str) -> bool {
    name.starts_with("MISTER_")
        && name.len() > 7
        && name[7..]
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn registered_by_prefix(name: &str, prefixes: &[RuntimeEnvironmentPrefix]) -> bool {
    prefixes.iter().any(|entry| {
        name.strip_prefix(&entry.prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= entry.max_suffix_length
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        })
    })
}

fn render_runtime_environment_reference(registry: &RuntimeEnvironmentRegistry) -> String {
    let mut output = String::from(
        "# MiSTer runtime environment reference\n\n<!-- Generated from apps/mister/config/runtime-environment.toml. Do not edit. -->\n\n",
    );
    output.push_str(&format!(
        "Registry format: `{}`. Baseline: {} literal occurrences, {} owned names, {} external/build-time names.\n\n",
        registry.format,
        registry.baseline.literal_occurrences,
        registry.baseline.unique_names,
        registry.baseline.external_build_names
    ));
    output.push_str("| Name | Classification | Shape | Default behavior | Visibility | Owner |\n|---|---|---|---|---|---|\n");
    for control in &registry.control {
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | `{}` |\n",
            control.name,
            control.classification,
            control.value_shape,
            control.default_behavior.replace('|', "\\|"),
            control.visibility,
            control.owner
        ));
    }
    output
}

fn main_fifo_source(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
        || path.starts_with("scripts")
        || path.starts_with("apps/mister/scripts")
        || path.starts_with(".github/workflows")
}

fn source_may_access_main_fifo(text: &str) -> bool {
    const ENDPOINTS: &[&str] = &["/dev/MiSTer_cmd", "/dev/MiSTer_cmd_reply"];
    let mut endpoint_names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        if ENDPOINTS.iter().any(|endpoint| line.contains(endpoint)) {
            if let Some(rest) = trimmed
                .strip_prefix("const ")
                .or_else(|| trimmed.strip_prefix("static "))
                && let Some((name, _)) = rest.split_once(':')
            {
                endpoint_names.push(name.trim().to_owned());
                continue;
            }
            if let Some((name, _)) = trimmed.split_once('=')
                && name
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_alphanumeric())
            {
                endpoint_names.push(name.trim().to_owned());
                continue;
            }
            if trimmed.contains("assert") || trimmed.contains("debug_assert") {
                continue;
            }
            if [
                "> /dev/MiSTer_cmd",
                ">/dev/MiSTer_cmd",
                "<>/dev/MiSTer_cmd",
                "< /dev/MiSTer_cmd",
                "</dev/MiSTer_cmd",
                "tee /dev/MiSTer_cmd",
                "of=/dev/MiSTer_cmd",
                ".open(\"/dev/MiSTer_cmd",
                "File::open(\"/dev/MiSTer_cmd",
                "File::create(\"/dev/MiSTer_cmd",
                "fs::write(\"/dev/MiSTer_cmd",
            ]
            .iter()
            .any(|pattern| line.contains(pattern))
            {
                return true;
            }
        }
    }
    endpoint_names.into_iter().any(|name| {
        [
            format!(".open({name})"),
            format!("File::open({name})"),
            format!("File::create({name})"),
            format!("fs::write({name}"),
            format!("std::fs::write({name}"),
            format!("send_mister_command({name}"),
            format!("> ${name}"),
            format!("> \"${name}\""),
            format!("> \"${{{name}}}\""),
            format!("tee ${name}"),
            format!("tee \"${name}\""),
            format!("of=${name}"),
        ]
        .iter()
        .any(|pattern| text.contains(pattern))
    })
}

fn repository_files(repository: &Path) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot enumerate policy files: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

fn read(repository: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(repository.join(path))
        .map_err(|error| format!("cannot read {path}: {error}"))
}

fn git_ignored(repository: &Path, path: &str) -> Result<bool, String> {
    Command::new("git")
        .args(["check-ignore", "-q", path])
        .current_dir(repository)
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("cannot inspect ignore policy: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn header_order_matches_platform_policy() {
        assert!(has_license_header(
            Path::new("src/lib.rs"),
            "// Copyright (C) 2026 Nigel Breslaw\n// SPDX-License-Identifier: GPL-3.0-or-later\n"
        ));
        assert!(has_license_header(
            Path::new("mister/platform/kernel/scanout-slots/a.c"),
            "// SPDX-License-Identifier: GPL-3.0-or-later\n// Copyright (C) 2026 Nigel Breslaw\n"
        ));
    }

    #[test]
    fn shell_policy_scans_tracked_and_untracked_but_not_ignored_files() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-shell-policy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("target/generated.txt"), "scripts/run-rust.sh\n").unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(check_shell_ownership(&root).is_ok());

        fs::create_dir_all(root.join("apps/mister/src")).unwrap();
        fs::write(
            root.join("apps/mister/src/contract.rs"),
            "const MAIN_COMMAND_ENDPOINT: &str = \"/dev/MiSTer_cmd\";\nenum Command { Launch }\n",
        )
        .unwrap();
        fs::write(
            root.join("apps/mister/src/launcher.rs"),
            "const CMD_FIFO: &str = \"/dev/MiSTer_cmd\";\nfn send() { OpenOptions::new().write(true).open(CMD_FIFO); }\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("agent-cli/src/host")).unwrap();
        fs::write(
            root.join("agent-cli/src/host/remote.rs"),
            "const COMMAND: &str = \"printf '%s\\n' command > /dev/MiSTer_cmd\";\n",
        )
        .unwrap();
        assert!(check_shell_ownership(&root).is_ok());

        fs::write(
            root.join("apps/mister/src/new_transport.rs"),
            "fn send() { OpenOptions::new().write(true).open(\"/dev/MiSTer_cmd\"); }\n",
        )
        .unwrap();
        let error = check_shell_ownership(&root).unwrap_err();
        assert!(error.contains("main_fifo_writer_outside_owner"));
        assert!(error.contains("mister/platform/runtime"));
        fs::remove_file(root.join("apps/mister/src/new_transport.rs")).unwrap();

        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/new-writer.sh"),
            "MAIN_FIFO=/dev/MiSTer_cmd\nprintf '%s\\n' command > \"$MAIN_FIFO\"\n",
        )
        .unwrap();
        let error = check_shell_ownership(&root).unwrap_err();
        assert!(error.contains("scripts/new-writer.sh"));
        fs::remove_file(root.join("scripts/new-writer.sh")).unwrap();

        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs/contract.md"), "scripts/run-rust.sh\n").unwrap();
        let error = check_shell_ownership(&root).unwrap_err();
        assert!(error.contains("docs/contract.md contains scripts/run-rust.sh"));

        fs::create_dir_all(root.join("history")).unwrap();
        fs::remove_file(root.join("docs/contract.md")).unwrap();
        fs::write(root.join("history/evidence.md"), "scripts/run-rust.sh\n").unwrap();
        assert!(check_shell_ownership(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_environment_registry_blocks_unregistered_controls() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-runtime-environment-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("apps/mister/src")).unwrap();
        fs::create_dir_all(root.join("apps/mister/config")).unwrap();
        fs::create_dir_all(root.join("docs/reference")).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let source = root.join("apps/mister/src/config.rs");
        fs::write(&source, "std::env::var(\"MISTER_ONE\");\n").unwrap();
        let registry_text = r#"format = "mister-magik-runtime-environment-v1"
source_roots = [
  "apps/mister/src",
  "mister/platform/runtime/src",
  "crates/catalog/src",
  "crates/particles/src",
  "crates/perf-events/src",
]

[baseline]
literal_occurrences = 1
unique_names = 1
external_build_names = 0

[[control]]
name = "MISTER_ONE"
owner = "apps/mister/src/config.rs"
classification = "production"
value_shape = "string"
default_behavior = "disabled"
visibility = "internal runtime"
"#;
        fs::write(
            root.join("apps/mister/config/runtime-environment.toml"),
            registry_text,
        )
        .unwrap();
        let mut registry: RuntimeEnvironmentRegistry = toml::from_str(registry_text).unwrap();
        registry
            .control
            .sort_by(|left, right| left.name.cmp(&right.name));
        fs::write(
            root.join("docs/reference/mister-runtime-environment.md"),
            render_runtime_environment_reference(&registry),
        )
        .unwrap();
        assert!(check_runtime_environment(&root).is_ok());

        fs::write(
            &source,
            "std::env::var(\"MISTER_ONE\");\nstd::env::var(\"MISTER_TWO\");\n",
        )
        .unwrap();
        let error = check_runtime_environment(&root).unwrap_err();
        assert!(error.contains("runtime_environment_unregistered: MISTER_TWO"));
        assert!(error.contains("owning module"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fragment_contract_reports_missing_before_forbidden_content() {
        assert!(require_fragments("workflow", "alpha beta", &["alpha"], &["gamma"]).is_ok());
        assert_eq!(
            require_fragments("workflow", "alpha", &["beta"], &["alpha"]).unwrap_err(),
            "workflow_contract_missing: beta"
        );
        assert_eq!(
            require_fragments("workflow", "alpha gamma", &["alpha"], &["gamma"]).unwrap_err(),
            "workflow_contract_forbidden: gamma"
        );
    }

    #[test]
    fn license_scope_excludes_generated_vendor_and_historical_content() {
        assert!(license_target(Path::new("src/lib.rs")));
        assert!(license_target(Path::new("Dockerfile.arm")));
        assert!(!license_target(Path::new("Cargo.lock")));
        assert!(!license_target(Path::new("history/old.rs")));
        assert!(!license_target(Path::new("apps/desktop/vendor/lib.rs")));
        assert!(!has_license_header(
            Path::new("src/lib.rs"),
            "// SPDX-License-Identifier: GPL-3.0-or-later\n"
        ));
        assert!(!has_license_header(
            Path::new("src/lib.rs"),
            "// SPDX-License-Identifier: GPL-3.0-or-later\n// Copyright (C) 2026 Nigel Breslaw\n"
        ));
    }
}
