// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::BuiltinOperation;
use crate::progress::{EventKind, Reporter};
use std::collections::{BTreeMap, BTreeSet};
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
        BuiltinOperation::PlatformManifestAuthority => "platform manifest authority",
        BuiltinOperation::DeviceCrateRootOwnership => "device crate-root ownership",
        BuiltinOperation::ExecutableBoundaries => "executable boundary ownership",
        BuiltinOperation::DistributionWorkflow => "distribution workflow",
        BuiltinOperation::KernelWorkflow => "kernel workflow",
        BuiltinOperation::PlatformWorkflow => "platform workflow",
        BuiltinOperation::ArchitectureWorkflow => "architecture trend workflow",
        BuiltinOperation::CiCache => "CI cache policy",
    }
}

pub fn run(operation: BuiltinOperation, repository: &Path) -> Result<(), String> {
    match operation {
        BuiltinOperation::AgentGuidance => check_agent_guidance(repository),
        BuiltinOperation::LicenseHeaders => check_license_headers(repository),
        BuiltinOperation::ShellOwnership => check_shell_ownership(repository),
        BuiltinOperation::RuntimeEnvironment => check_runtime_environment(repository),
        BuiltinOperation::PlatformManifestAuthority => {
            check_platform_manifest_authority(repository)
        }
        BuiltinOperation::DeviceCrateRootOwnership => check_device_crate_root_ownership(repository),
        BuiltinOperation::ExecutableBoundaries => check_executable_boundaries(repository),
        BuiltinOperation::DistributionWorkflow => check_distribution_workflow(repository),
        BuiltinOperation::KernelWorkflow => check_kernel_workflow(repository),
        BuiltinOperation::PlatformWorkflow => check_platform_workflow(repository),
        BuiltinOperation::ArchitectureWorkflow => check_architecture_workflow(repository),
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

fn check_architecture_workflow(repository: &Path) -> Result<(), String> {
    let workflow = read(repository, ".github/workflows/architecture-trends.yml")?;
    require_fragments(
        "architecture_workflow",
        &workflow,
        &[
            "name: Architecture Trends",
            "pull_request:",
            "permissions:\n  contents: read",
            "ref: ${{ github.event.pull_request.head.sha }}",
            "fetch-depth: 0",
            "BASE_SHA: ${{ github.event.pull_request.base.sha }}",
            "HEAD_SHA: ${{ github.event.pull_request.head.sha }}",
            "git cat-file -e \"${BASE_SHA}^{commit}\"",
            "git fetch --no-tags --depth=1 origin \"$BASE_SHA\"",
            "continue-on-error: true",
            "scripts/agent architecture report",
            "--base \"$BASE_SHA\"",
            "--head \"$HEAD_SHA\"",
            "$GITHUB_STEP_SUMMARY",
            "actions/upload-artifact@v7",
            "architecture-report.json",
            "if-no-files-found: warn",
        ],
        &[
            "push:",
            "workflow_dispatch:",
            "contents: write",
            "issues: write",
            "pull-requests: write",
            "actions/github-script",
            "gh pr comment",
            "scripts/agent build",
            "scripts/agent benchmark",
            "scripts/agent deliver",
            "scripts/agent device",
        ],
    )
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
        "docs/agents/ai-efficiency.md",
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
    let efficiency = read(repository, "docs/agents/ai-efficiency.md")?;
    for expected in [
        "1,200 tokens",
        "3,000-token",
        "$magik-rust-lsp",
        "150 lines",
        "100 matches",
        "Never forward unconditional broad `r.output`",
        "scripts/codex-context-report.py",
    ] {
        if !efficiency.contains(expected) {
            return Err(format!("ai_efficiency_guidance_missing: {expected}"));
        }
    }
    check_codex_config(repository)?;
    check_retired_validation_call_sites(repository)
}

fn check_codex_config(repository: &Path) -> Result<(), String> {
    let text = read(repository, ".codex/config.toml")?;
    let config: toml::Value =
        toml::from_str(&text).map_err(|error| format!("codex_config_invalid: {error}"))?;
    let table = config
        .as_table()
        .ok_or_else(|| "codex_config_invalid: root must be a table".to_string())?;
    let required = [
        ("allow_login_shell", toml::Value::Boolean(false)),
        ("tool_output_token_limit", toml::Value::Integer(3000)),
        (
            "model_auto_compact_token_limit_scope",
            toml::Value::String("body_after_prefix".into()),
        ),
        ("model_reasoning_effort", toml::Value::String("high".into())),
        (
            "model_reasoning_summary",
            toml::Value::String("concise".into()),
        ),
        ("model_verbosity", toml::Value::String("low".into())),
    ];
    for (key, expected) in required {
        let Some(actual) = table.get(key) else {
            return Err(format!("codex_config_missing: {key}"));
        };
        if std::mem::discriminant(actual) != std::mem::discriminant(&expected) {
            return Err(format!("codex_config_type: {key}"));
        }
        if actual != &expected {
            return Err(format!("codex_config_value: {key}"));
        }
    }
    for forbidden in ["model_context_window", "model_auto_compact_token_limit"] {
        if table.contains_key(forbidden) {
            return Err(format!("codex_config_forbidden: {forbidden}"));
        }
    }
    Ok(())
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
            "mister/platform/runtime/src/main_command.rs",
            "production app command transport",
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
                "main_fifo_writer_outside_owner: {relative}; production app command transport targets mister/platform/runtime; approved boundary owners: {owners}"
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
    #[serde(default)]
    parser: Option<String>,
    #[serde(default)]
    typed_default: Option<toml::Value>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    sensitivity: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    documentation: Option<RuntimeEnvironmentDocumentation>,
}

#[derive(Clone, serde::Deserialize)]
struct RuntimeEnvironmentDocumentation {
    summary: String,
    #[serde(default)]
    accepted_values: Vec<String>,
    value_policy: String,
}

fn check_runtime_environment(repository: &Path) -> Result<(), String> {
    const REGISTRY_PATH: &str = "apps/mister/config/runtime-environment.toml";
    const REFERENCE_PATH: &str = "docs/reference/mister-runtime-environment.md";
    const FORMATS: &[&str] = &[
        "mister-magik-runtime-environment-v1",
        "mister-magik-runtime-environment-v2",
    ];
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
    if !FORMATS.contains(&registry.format.as_str()) {
        return Err(format!(
            "runtime_environment_format_invalid: expected {}",
            FORMATS.join(" or ")
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
            || validate_runtime_environment_metadata(control).is_err()
        {
            return Err(format!(
                "runtime_environment_control_invalid: {}",
                control.name
            ));
        }
    }
    let mut accepted_names = registered.clone();
    for control in &registry.control {
        for alias in &control.aliases {
            if !valid_environment_name(alias) || !accepted_names.insert(alias.clone()) {
                return Err(format!(
                    "runtime_environment_alias_invalid: {} alias {}",
                    control.name, alias
                ));
            }
        }
    }
    for control in &registry.control {
        if control
            .conflicts
            .iter()
            .any(|conflict| conflict == &control.name || !accepted_names.contains(conflict))
        {
            return Err(format!(
                "runtime_environment_conflict_invalid: {}",
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
        .iter()
        .filter(|name| !accepted_names.contains(*name))
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

fn validate_runtime_environment_metadata(control: &RuntimeEnvironmentControl) -> Result<(), ()> {
    const PARSERS: &[&str] = &[
        "bool",
        "i64",
        "u64",
        "f64",
        "string",
        "path",
        "path-list",
        "enum",
    ];
    const SCOPES: &[&str] = &["process", "command", "instrumentation", "external", "build"];
    const SENSITIVITY: &[&str] = &["public", "path", "secret", "volatile-token"];
    if control
        .parser
        .as_deref()
        .is_some_and(|parser| !PARSERS.contains(&parser))
        || control
            .scope
            .as_deref()
            .is_some_and(|scope| !SCOPES.contains(&scope))
        || control
            .sensitivity
            .as_deref()
            .is_some_and(|value| !SENSITIVITY.contains(&value))
        || control
            .typed_default
            .as_ref()
            .is_some_and(|value| !typed_default_matches(control.parser.as_deref(), value))
        || control.typed_default.is_some() && control.parser.is_none()
    {
        return Err(());
    }
    if control.documentation.as_ref().is_some_and(|documentation| {
        documentation.summary.trim().is_empty()
            || !["document", "redact", "omit"].contains(&documentation.value_policy.as_str())
            || documentation
                .accepted_values
                .iter()
                .any(|value| value.trim().is_empty())
            || documentation
                .accepted_values
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != documentation.accepted_values.len()
    }) {
        return Err(());
    }
    Ok(())
}

fn typed_default_matches(parser: Option<&str>, value: &toml::Value) -> bool {
    match parser {
        Some("bool") => value.is_bool(),
        Some("i64") => value.is_integer(),
        Some("u64") => value.as_integer().is_some_and(|value| value >= 0),
        Some("f64") => value.is_float(),
        Some("string" | "path" | "enum") => value.is_str(),
        Some("path-list") => value
            .as_array()
            .is_some_and(|values| values.iter().all(toml::Value::is_str)),
        _ => false,
    }
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
    output.push_str("| Name | Classification | Shape | Default behavior | Parser | Typed default | Scope | Conflicts | Sensitivity | Aliases | Documentation | Visibility | Owner |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for control in &registry.control {
        let documentation = control
            .documentation
            .as_ref()
            .map(|documentation| {
                let accepted = if documentation.accepted_values.is_empty() {
                    String::new()
                } else {
                    format!("; values: {}", documentation.accepted_values.join(", "))
                };
                format!(
                    "{}{}; value policy: {}",
                    documentation.summary, accepted, documentation.value_policy
                )
            })
            .unwrap_or_else(|| "—".into());
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | `{}` |\n",
            control.name,
            control.classification,
            control.value_shape,
            control.default_behavior.replace('|', "\\|"),
            control.parser.as_deref().unwrap_or("—"),
            control
                .typed_default
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "—".into())
                .replace('|', "\\|"),
            control.scope.as_deref().unwrap_or("—"),
            metadata_list(&control.conflicts),
            control.sensitivity.as_deref().unwrap_or("—"),
            metadata_list(&control.aliases),
            documentation.replace('|', "\\|"),
            control.visibility,
            control.owner
        ));
    }
    output
}

fn metadata_list(values: &[String]) -> String {
    if values.is_empty() {
        "—".into()
    } else {
        values.join(", ")
    }
}

fn check_device_crate_root_ownership(repository: &Path) -> Result<(), String> {
    const EXPECTED: [(&str, &str); 0] = [];
    let main = read(repository, "apps/mister/src/main.rs")?;
    let app_entry = read(repository, "apps/mister/src/app_entry.rs")?;
    let library = read(repository, "apps/mister/src/lib.rs")?;
    let experiments = fs::read_to_string(repository.join("apps/mister/src/experiments/mod.rs"))
        .unwrap_or_default();
    let main_modules = rust_module_declarations(&main);
    let library_modules = rust_module_declarations(&library);
    let mut actual: BTreeSet<String> = main_modules
        .intersection(&library_modules)
        .cloned()
        .collect();
    if main_modules.contains("experiments")
        && library.contains("pub mod experiments {")
        && library.contains("pub mod effects;")
        && experiments.contains("mod effects;")
    {
        actual.insert("experiments/effects".into());
    }
    let expected: BTreeSet<String> = EXPECTED
        .iter()
        .map(|(module, _batch)| (*module).to_owned())
        .collect();
    if actual != expected {
        let unexpected = actual.difference(&expected).cloned().collect::<Vec<_>>();
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        return Err(format!(
            "device_crate_root_inventory_drift: unexpected={} missing={}; migrate through the recorded 20a-20d batches",
            unexpected.join(","),
            missing.join(",")
        ));
    }
    if main.lines().count() > 24
        || !main.contains("#[global_allocator]")
        || !main.contains("device_layout::initialize_process_env()")
        || !main.contains("app_entry::run()")
        || !main_modules.is_empty()
        || main.contains("std::env::args")
        || main.contains("#![allow(dead_code)]")
        || app_entry.contains("#![allow(dead_code)]")
    {
        return Err(
            "device_binary_bootstrap_drift: main.rs owns only allocator, earliest layout initialization, and app_entry::run"
                .into(),
        );
    }
    Ok(())
}

fn check_executable_boundaries(repository: &Path) -> Result<(), String> {
    let portable = read(repository, "crates/magik-core/src/launcher_effects.rs")?;
    require_fragments(
        "portable_launcher_effects",
        &portable,
        &[
            "pub trait LaunchHandoff",
            "pub trait DisplayControl",
            "pub trait RuntimeState",
            "pub trait LauncherPersistence",
        ],
        &[
            "std::fs",
            "std::path",
            "std::process",
            "slint::",
            "/dev/",
            "MainCommand",
        ],
    )?;

    let runtime = read(repository, "mister/platform/runtime/src/lib.rs")?;
    require_fragments(
        "platform_runtime_effects",
        &runtime,
        &[
            "pub mod display_control;",
            "pub mod direct_reset_fault;",
            "pub mod main_command;",
            "pub mod runtime_state;",
        ],
        &[],
    )?;
    require_fragments(
        "platform_display_control",
        &read(repository, "mister/platform/runtime/src/display_control.rs")?,
        &[
            "pub struct MainDisplayControl",
            "impl DisplayControl for MainDisplayControl",
            "main_command::execute(&MainCommand::DisplayApply",
            "pub fn parse_state_response",
        ],
        &[],
    )?;
    require_fragments(
        "platform_runtime_state",
        &read(repository, "mister/platform/runtime/src/runtime_state.rs")?,
        &[
            "pub struct SystemRuntimeState",
            "impl RuntimeState for SystemRuntimeState",
        ],
        &[],
    )?;
    require_fragments(
        "platform_direct_reset",
        &read(
            repository,
            "mister/platform/runtime/src/direct_reset_fault.rs",
        )?,
        &[
            "pub struct SystemDirectResetFaultControl",
            "impl DirectResetFaultControl for SystemDirectResetFaultControl",
        ],
        &[],
    )?;

    let launcher = read(repository, "apps/mister/src/launcher.rs")?;
    require_fragments(
        "launcher_effect_composition",
        &launcher,
        &[
            "LaunchHandoff for LaunchIoHandoff",
            "impl LauncherPersistence for SystemLauncherPersistence",
            "SystemRuntimeState",
            "display_control::MainDisplayControl",
        ],
        &[
            "LauncherDisplayControl",
            "MagikPlatform",
            "MisterRuntimeBackend",
            "MisterRuntime",
        ],
    )?;

    let device_agent = read(repository, "mister/tools/agent/src/main.rs")?;
    let cli = read(repository, "agent-cli/src/main.rs")?;
    if executable_error_flattening_returned(&device_agent, &cli) {
        return Err("executable_error_string_flattening_returned".into());
    }
    require_fragments(
        "device_failure_emission",
        &device_agent,
        &[
            "fn failure_response(",
            "fn authentication_failure_response(",
            "fn busy_failure_response(",
            "fn unavailable_failure_response(",
            "fn operation_failure_response(",
            "fn alpha_candidate_failure_response(",
        ],
        &[],
    )?;
    let error = read(repository, "agent-cli/src/error.rs")?;
    require_fragments(
        "agent_error_structure",
        &error,
        &[
            "StructuredDevice",
            "pub fn structured_failure(&self)",
            "Self::Phase { source, .. } | Self::Cancelled(source) => source.structured_failure()",
        ],
        &[],
    )?;
    require_fragments(
        "agent_cli_error_boundary",
        &cli,
        &[
            "fn run() -> AgentResult<ExitCode>",
            "fn open() -> AgentResult<Self>",
            "reporter.emit_failure(\"request\", &error)",
            "ExitCode::from(70)",
            "ExitCode::FAILURE",
            "ExitCode::from(3)",
        ],
        &[
            "fn run() -> Result<ExitCode, String>",
            "reporter.emit(EventKind::Failed, \"request\", &error.to_string(), None)",
        ],
    )?;
    let progress = read(repository, "agent-cli/src/progress.rs")?;
    require_fragments(
        "agent_failure_projection",
        &progress,
        &[
            "pub struct FailureEvidence",
            "pub code: String",
            "pub phase: String",
            "pub retry_policy: String",
            "pub recovery_required: bool",
            "pub fn emit_failure(",
        ],
        &[],
    )?;
    let failure_projection = progress
        .split_once("pub struct FailureEvidence")
        .and_then(|(_, tail)| tail.split_once('}'))
        .map(|(body, _)| body)
        .ok_or("agent_failure_projection_missing")?;
    if failure_projection.contains("detail") {
        return Err("agent_failure_projection_must_redact_detail".into());
    }
    require_fragments(
        "agent_failure_evidence",
        &read(repository, "agent-cli/src/evidence.rs")?,
        &[
            "failure_json TEXT",
            "PRAGMA user_version = 12",
            "pub failure: Option<FailureEvidence>",
            "fn migrate_v11_to_v12",
        ],
        &[],
    )?;
    Ok(())
}

fn executable_error_flattening_returned(device_agent: &str, cli: &str) -> bool {
    let device_agent = device_agent.split_whitespace().collect::<String>();
    let cli = cli.split_whitespace().collect::<String>();
    device_agent.contains("response(id,false")
        || device_agent.contains("response(None,false")
        || cli.contains("fnrun()->Result<ExitCode,String>")
        || cli.contains("reporter.emit(EventKind::Failed,\"request\",&error.to_string(),None)")
}

fn rust_module_declarations(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let declaration = line
                .strip_prefix("mod ")
                .or_else(|| line.strip_prefix("pub mod "))
                .or_else(|| line.strip_prefix("pub(crate) mod "))?;
            declaration
                .strip_suffix(';')
                .filter(|name| {
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
                })
                .map(str::to_owned)
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct PlatformManifestSchema {
    format: String,
    manifest_format: String,
    file_name: String,
    latch_protocol_version: String,
    latch_capability_mask: String,
    contract_crate: String,
    behavioral_authority: String,
    fields: Vec<String>,
    layouts: BTreeMap<String, PlatformManifestLayout>,
}

#[derive(serde::Deserialize)]
struct PlatformManifestLayout {
    root: String,
    main: String,
    gui: String,
    manager: String,
    scanout_module: String,
    scanout_metadata: String,
    latch_rbf: String,
    latch_metadata: String,
}

fn check_platform_manifest_authority(repository: &Path) -> Result<(), String> {
    const SCHEMA_PATH: &str = "mister/platform/contracts/platform-v3.schema.toml";
    const SCHEMA_FORMAT: &str = "mister-magik-platform-v3-schema-v1";
    const AUTHORITY: &str = "mister/platform/contracts/manifest/src/lib.rs";
    let text = read(repository, SCHEMA_PATH)?;
    let schema: PlatformManifestSchema = toml::from_str(&text)
        .map_err(|error| format!("platform_manifest_schema_invalid: {error}"))?;
    if schema.format != SCHEMA_FORMAT
        || schema.behavioral_authority != AUTHORITY
        || schema.contract_crate != "platform-manifest-contract"
        || schema
            .layouts
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ["dev", "public"]
    {
        return Err("platform_manifest_schema_identity_invalid".into());
    }
    let contract_manifest = read(repository, "mister/platform/contracts/manifest/Cargo.toml")?;
    let contract_build = read(repository, "mister/platform/contracts/manifest/build.rs")?;
    let contract_api = read(repository, AUTHORITY)?;
    if !contract_manifest.contains("mister-magik-platform-manifest-contract")
        || !contract_build.contains("../platform-v3.schema.toml")
        || !contract_build.contains("pub const FIELDS")
        || !contract_build.contains("pub const PUBLIC_PATHS")
        || !contract_build.contains("pub const DEVELOPMENT_PATHS")
        || !contract_api.contains("ValidationProfile")
        || !contract_api.contains("qualification_candidate_id")
    {
        return Err("platform_manifest_contract_authority_invalid".into());
    }
    let latch: serde_json::Value = serde_json::from_str(&read(
        repository,
        "mister/platform/fpga/menu-vblank-latch/latch-protocol.json",
    )?)
    .map_err(|error| format!("latch_protocol_invalid: {error}"))?;
    let active_version = latch["active_protocol_version"]
        .as_u64()
        .ok_or("latch_protocol_version_missing")?;
    let flags = latch["protocols"][active_version.to_string()]["flags"]
        .as_u64()
        .ok_or("latch_capability_flags_missing")?;
    if schema.latch_protocol_version != active_version.to_string()
        || schema.latch_capability_mask != format!("0x{flags:04x}")
    {
        return Err("platform_manifest_latch_contract_drift".into());
    }

    check_generated_platform_manifest_consumers(repository, &schema)?;
    let public = &schema.layouts["public"];
    let dev = &schema.layouts["dev"];
    let files = repository_files(repository)?;
    let mut unledgered = Vec::new();
    let mut installed_path_duplicates = Vec::new();
    for path in files {
        let relative = path.to_string_lossy();
        if relative == AUTHORITY
            || relative == SCHEMA_PATH
            || relative.starts_with("mister/platform/contracts/manifest/")
            || relative.starts_with("mister/platform/contracts/generated/")
            || relative == "agent-cli/src/checks.rs"
            || relative.starts_with("docs/")
            || relative.starts_with("history/")
            || relative.starts_with("reference/")
        {
            continue;
        }
        let extension = path.extension().and_then(|value| value.to_str());
        if !matches!(extension, Some("rs" | "sh" | "py" | "yml" | "yaml")) {
            continue;
        }
        let Ok(source) = fs::read_to_string(repository.join(&path)) else {
            continue;
        };
        if extension == Some("rs")
            && platform_installed_path_duplicate(&source, &schema, public, dev)
        {
            installed_path_duplicates.push(relative.clone().into_owned());
        }
        if platform_manifest_structural_duplicate(&source, &schema, public, dev) {
            unledgered.push(relative.into_owned());
        }
    }
    if !installed_path_duplicates.is_empty() {
        installed_path_duplicates.sort();
        return Err(format!(
            "platform_installed_path_duplicate_outside_contract: {}; use Layout::paths()",
            installed_path_duplicates.join(",")
        ));
    }
    if !unledgered.is_empty() {
        unledgered.sort();
        return Err(format!(
            "platform_manifest_duplicate_outside_contract: {}; adopt mister/platform/contracts/platform-v3.schema.toml",
            unledgered.join(",")
        ));
    }
    Ok(())
}

fn platform_installed_path_duplicate(
    source: &str,
    schema: &PlatformManifestSchema,
    public: &PlatformManifestLayout,
    dev: &PlatformManifestLayout,
) -> bool {
    let production = source.split("#[cfg(test)]").next().unwrap_or(source);
    let public_manifest = format!("{}/{}", public.root, schema.file_name);
    let dev_manifest = format!("{}/{}", dev.root, schema.file_name);
    [
        public_manifest.as_str(),
        dev_manifest.as_str(),
        &public.main,
        &public.gui,
        &public.manager,
        &public.scanout_module,
        &public.scanout_metadata,
        &public.latch_rbf,
        &public.latch_metadata,
        &dev.main,
        &dev.gui,
        &dev.manager,
        &dev.scanout_module,
        &dev.scanout_metadata,
        &dev.latch_rbf,
        &dev.latch_metadata,
    ]
    .iter()
    .any(|path| production.contains(*path))
}

fn check_generated_platform_manifest_consumers(
    repository: &Path,
    schema: &PlatformManifestSchema,
) -> Result<(), String> {
    const GENERATED_ROOT: &str = "mister/platform/contracts/generated";
    type PlatformPathSelector = fn(&PlatformManifestLayout) -> &str;
    const COMPONENTS: [(&str, PlatformPathSelector); 8] = [
        ("ROOT", |layout| &layout.root),
        ("MAIN", |layout| &layout.main),
        ("GUI", |layout| &layout.gui),
        ("MANAGER", |layout| &layout.manager),
        ("SCANOUT_MODULE", |layout| &layout.scanout_module),
        ("SCANOUT_METADATA", |layout| &layout.scanout_metadata),
        ("LATCH_RBF", |layout| &layout.latch_rbf),
        ("LATCH_METADATA", |layout| &layout.latch_metadata),
    ];
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
    let mut expected = String::from(
        "# Copyright (C) 2026 Nigel Breslaw\n# SPDX-License-Identifier: GPL-3.0-or-later\n\n# @generated by scripts/checks/generate-platform-v3-consumers.py; do not edit.\n",
    );
    for (name, value) in [
        ("FORMAT", schema.manifest_format.as_str()),
        ("FILE_NAME", schema.file_name.as_str()),
    ] {
        expected.push_str(&format!("PLATFORM_V3_{name}={}\n", quote(value)));
    }
    expected.push_str(&format!(
        "PLATFORM_V3_FIELD_NAMES={}\n",
        quote(&schema.fields.join(" "))
    ));
    for (name, layout) in [
        ("PUBLIC", &schema.layouts["public"]),
        ("DEV", &schema.layouts["dev"]),
    ] {
        for (component, value) in COMPONENTS {
            expected.push_str(&format!(
                "PLATFORM_V3_{name}_{component}={}\n",
                quote(value(layout))
            ));
        }
    }
    let constants = read(
        repository,
        &format!("{GENERATED_ROOT}/platform-v3.constants.sh"),
    )?;
    if constants != expected {
        return Err("platform_manifest_generated_constants_stale".into());
    }

    for (file, layout) in [
        (
            "platform-v3.public.fixture",
            mister_magik_platform_manifest_contract::Layout::Public,
        ),
        (
            "platform-v3.development.fixture",
            mister_magik_platform_manifest_contract::Layout::Development,
        ),
    ] {
        let fixture = read(repository, &format!("{GENERATED_ROOT}/{file}"))?;
        let parsed = mister_magik_platform_manifest_contract::parse(
            &fixture,
            layout,
            mister_magik_platform_manifest_contract::ValidationProfile::AgentStrict,
        )
        .map_err(|error| format!("platform_manifest_generated_fixture_invalid: {file}: {error}"))?;
        if mister_magik_platform_manifest_contract::serialize(parsed.values()).map_err(|error| {
            format!("platform_manifest_generated_fixture_invalid: {file}: {error}")
        })? != fixture
        {
            return Err(format!("platform_manifest_generated_fixture_stale: {file}"));
        }
    }
    Ok(())
}

fn platform_manifest_structural_duplicate(
    source: &str,
    schema: &PlatformManifestSchema,
    public: &PlatformManifestLayout,
    dev: &PlatformManifestLayout,
) -> bool {
    if source.contains("mister_magik_platform_manifest_contract::parse(")
        || source.contains("mister_magik_platform_manifest_contract::serialize(")
        || (source.contains("mister_magik_platform_manifest_contract")
            && source.contains("platform_manifest_contract::parse("))
        || (source.contains("mister_magik_platform_manifest_contract")
            && source.contains("contract::parse("))
        || source.contains("mister/platform/contracts/generated/")
        || source.contains("generate-platform-v3-consumers.py")
    {
        return false;
    }
    let field_count = schema
        .fields
        .iter()
        .filter(|field| source.contains(field.as_str()))
        .count();
    source.contains(&schema.manifest_format)
        || field_count >= 12
        || (source.contains(&public.root)
            && source.contains(&dev.root)
            && source.contains(&public.main)
            && source.contains(&dev.main))
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
    fn codex_config_contract_accepts_required_defaults_and_unrelated_tables() {
        let root = temporary_root("codex-config-valid");
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(
            root.join(".codex/config.toml"),
            concat!(
                "allow_login_shell = false\n",
                "tool_output_token_limit = 3000\n",
                "model_auto_compact_token_limit_scope = \"body_after_prefix\"\n",
                "model_reasoning_effort = \"high\"\n",
                "model_reasoning_summary = \"concise\"\n",
                "model_verbosity = \"low\"\n",
                "[mcp_servers.fixture]\n",
                "command = \"fixture\"\n",
            ),
        )
        .unwrap();
        assert!(check_codex_config(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_config_contract_rejects_missing_typed_valued_and_forbidden_keys() {
        let valid = concat!(
            "allow_login_shell = false\n",
            "tool_output_token_limit = 3000\n",
            "model_auto_compact_token_limit_scope = \"body_after_prefix\"\n",
            "model_reasoning_effort = \"high\"\n",
            "model_reasoning_summary = \"concise\"\n",
            "model_verbosity = \"low\"\n",
        );
        for (label, fixture, expected) in [
            (
                "missing",
                valid.replace("model_verbosity = \"low\"\n", ""),
                "codex_config_missing: model_verbosity",
            ),
            (
                "type",
                valid.replace(
                    "tool_output_token_limit = 3000",
                    "tool_output_token_limit = \"3000\"",
                ),
                "codex_config_type: tool_output_token_limit",
            ),
            (
                "value",
                valid.replace(
                    "model_reasoning_effort = \"high\"",
                    "model_reasoning_effort = \"medium\"",
                ),
                "codex_config_value: model_reasoning_effort",
            ),
            (
                "forbidden",
                format!("{valid}model_context_window = 1050000\n"),
                "codex_config_forbidden: model_context_window",
            ),
        ] {
            let root = temporary_root(&format!("codex-config-{label}"));
            fs::create_dir_all(root.join(".codex")).unwrap();
            fs::write(root.join(".codex/config.toml"), fixture).unwrap();
            assert_eq!(check_codex_config(&root).unwrap_err(), expected);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-cli-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
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
        fs::create_dir_all(root.join("mister/platform/runtime/src")).unwrap();
        fs::write(
            root.join("mister/platform/runtime/src/main_command.rs"),
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
            root.join("apps/mister/src/launcher.rs"),
            "fn send() { OpenOptions::new().write(true).open(\"/dev/MiSTer_cmd\"); }\n",
        )
        .unwrap();
        let error = check_shell_ownership(&root).unwrap_err();
        assert!(error.contains("apps/mister/src/launcher.rs"));
        fs::remove_file(root.join("apps/mister/src/launcher.rs")).unwrap();

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
    fn runtime_environment_v2_accepts_every_parser_and_scope() {
        let fixtures = [
            ("bool", "true", "process"),
            ("i64", "-2", "command"),
            ("u64", "2", "instrumentation"),
            ("f64", "1.5", "external"),
            ("string", "\"value\"", "build"),
            ("path", "\"/tmp/value\"", "process"),
            ("path-list", "[\"/tmp/one\", \"/tmp/two\"]", "command"),
            ("enum", "\"auto\"", "instrumentation"),
        ];
        for (parser, default, scope) in fixtures {
            let fixture = format!(
                r#"name = "MISTER_FIXTURE"
owner = "apps/mister/src/config.rs"
classification = "test"
value_shape = "fixture"
default_behavior = "fixture"
visibility = "fixture"
parser = "{parser}"
typed_default = {default}
scope = "{scope}"
conflicts = ["MISTER_OTHER"]
sensitivity = "public"
aliases = ["MISTER_FIXTURE_ALIAS"]
documentation = {{ summary = "Fixture metadata", accepted_values = ["one", "two"], value_policy = "document" }}
"#
            );
            let control: RuntimeEnvironmentControl = toml::from_str(&fixture).unwrap();
            assert!(
                validate_runtime_environment_metadata(&control).is_ok(),
                "metadata fixture rejected for parser={parser} scope={scope}"
            );
        }

        let invalid: RuntimeEnvironmentControl = toml::from_str(
            r#"name = "MISTER_FIXTURE"
owner = "apps/mister/src/config.rs"
classification = "test"
value_shape = "fixture"
default_behavior = "fixture"
visibility = "fixture"
parser = "bool"
typed_default = "not-a-bool"
scope = "process"
"#,
        )
        .unwrap();
        assert!(validate_runtime_environment_metadata(&invalid).is_err());
    }

    #[test]
    fn platform_manifest_schema_rejects_an_unledgered_duplicate() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-platform-manifest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("apps/mister/src")).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for relative in [
            "mister/platform/contracts/platform-v3.schema.toml",
            "mister/platform/contracts/manifest/Cargo.toml",
            "mister/platform/contracts/manifest/build.rs",
            "mister/platform/contracts/manifest/src/lib.rs",
            "mister/platform/contracts/generated/platform-v3.constants.sh",
            "mister/platform/contracts/generated/platform-v3.public.fixture",
            "mister/platform/contracts/generated/platform-v3.development.fixture",
            "mister/platform/fpga/menu-vblank-latch/latch-protocol.json",
        ] {
            let destination = root.join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(repository.join(relative), destination).unwrap();
        }
        fs::write(
            root.join("apps/mister/src/new_manifest.rs"),
            "const FORMAT: &str = \"mister-magik-platform-v3\";\n",
        )
        .unwrap();
        let error = check_platform_manifest_authority(&root).unwrap_err();
        assert!(error.contains("platform_manifest_duplicate_outside_contract"));
        assert!(error.contains("apps/mister/src/new_manifest.rs"));
        fs::write(
            root.join("apps/mister/src/new_manifest.rs"),
            "use mister_magik_platform_manifest_contract::ValidationProfile;\nconst FORMAT: &str = \"mister-magik-platform-v3\";\n",
        )
        .unwrap();
        let error = check_platform_manifest_authority(&root).unwrap_err();
        assert!(error.contains("apps/mister/src/new_manifest.rs"));
        fs::write(
            root.join("apps/mister/src/new_manifest.rs"),
            "fn parse(text: &str) { let _ = mister_magik_platform_manifest_contract::parse(text, layout, profile); }\nconst FORMAT: &str = \"mister-magik-platform-v3\";\n",
        )
        .unwrap();
        assert!(check_platform_manifest_authority(&root).is_ok());
        fs::write(
            root.join("apps/mister/src/new_manifest.rs"),
            "fn fixture(values: &Values) { let _ = mister_magik_platform_manifest_contract::serialize(values); }\nconst FORMAT: &str = \"mister-magik-platform-v3\";\n",
        )
        .unwrap();
        assert!(check_platform_manifest_authority(&root).is_ok());
        fs::remove_file(root.join("apps/mister/src/new_manifest.rs")).unwrap();
        fs::write(
            root.join("apps/mister/src/media_manifest.rs"),
            "const FORMAT: &str = \"mister-magik-media-v1\";\n",
        )
        .unwrap();
        assert!(check_platform_manifest_authority(&root).is_ok());
        fs::remove_file(root.join("apps/mister/src/media_manifest.rs")).unwrap();
        fs::write(
            root.join("apps/mister/src/copied_layout.rs"),
            "const GUI: &str = \"/media/fat/mister-magik/mister-magik-fb\";\n",
        )
        .unwrap();
        let error = check_platform_manifest_authority(&root).unwrap_err();
        assert!(error.contains("platform_installed_path_duplicate_outside_contract"));
        assert!(error.contains("apps/mister/src/copied_layout.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn device_crate_root_guard_rejects_a_duplicate_outside_the_inventory() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-device-root-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("apps/mister/src/experiments")).unwrap();
        fs::write(
            root.join("apps/mister/src/main.rs"),
            "#[global_allocator]\nstatic ALLOC: Alloc = Alloc;\nfn main() { unsafe { device_layout::initialize_process_env() }; app_entry::run(); }\n",
        )
        .unwrap();
        fs::write(
            root.join("apps/mister/src/lib.rs"),
            "pub mod library_only;\n",
        )
        .unwrap();
        fs::write(
            root.join("apps/mister/src/app_entry.rs"),
            "pub fn run() {}\n",
        )
        .unwrap();
        fs::write(root.join("apps/mister/src/experiments/mod.rs"), "").unwrap();
        assert!(check_device_crate_root_ownership(&root).is_ok());
        fs::write(
            root.join("apps/mister/src/main.rs"),
            "#[global_allocator]\nstatic ALLOC: Alloc = Alloc;\nmod unplanned;\nfn main() { unsafe { device_layout::initialize_process_env() }; app_entry::run(); }\n",
        )
        .unwrap();
        fs::write(
            root.join("apps/mister/src/lib.rs"),
            "pub mod library_only;\nmod unplanned;\n",
        )
        .unwrap();
        let error = check_device_crate_root_ownership(&root).unwrap_err();
        assert!(error.contains("unexpected=unplanned"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn executable_error_guard_rejects_legacy_string_edges() {
        assert!(!executable_error_flattening_returned(
            "failure_response(id, error)",
            "fn run() -> AgentResult<ExitCode> { reporter.emit_failure(\"request\", &error); }",
        ));
        assert!(executable_error_flattening_returned(
            "response(id, false, None, Some(error))",
            "fn run() -> AgentResult<ExitCode> { Ok(ExitCode::SUCCESS) }",
        ));
        assert!(executable_error_flattening_returned(
            "failure_response(id, error)",
            "fn run() -> Result<ExitCode, String> { reporter.emit(EventKind::Failed, \"request\", &error.to_string(), None); }",
        ));
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
    fn architecture_workflow_contract_requires_deterministic_base_recovery() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-architecture-workflow-{}",
            std::process::id()
        ));
        let workflow = root.join(".github/workflows");
        fs::create_dir_all(&workflow).unwrap();
        let fixture = include_str!("../../.github/workflows/architecture-trends.yml").replace(
            "git fetch --no-tags --depth=1 origin \"$BASE_SHA\"",
            "echo missing deterministic base recovery",
        );
        fs::write(workflow.join("architecture-trends.yml"), fixture).unwrap();
        let error = check_architecture_workflow(&root).unwrap_err();
        assert!(error.contains("git fetch --no-tags --depth=1 origin \"$BASE_SHA\""));
        fs::remove_dir_all(root).unwrap();
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
