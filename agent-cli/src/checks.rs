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
        BuiltinOperation::GitIdentity => "Git identity",
        BuiltinOperation::StagedGitPolicy => "staged Git policy",
        BuiltinOperation::LicenseHeaders => "license headers",
        BuiltinOperation::ShellOwnership => "shell ownership",
        BuiltinOperation::DistributionWorkflow => "distribution workflow",
        BuiltinOperation::KernelWorkflow => "kernel workflow",
        BuiltinOperation::PlatformWorkflow => "platform workflow",
        BuiltinOperation::CiCache => "CI cache policy",
    }
}

pub fn run(operation: BuiltinOperation, repository: &Path) -> Result<(), String> {
    match operation {
        BuiltinOperation::AgentGuidance => check_agent_guidance(repository),
        BuiltinOperation::GitIdentity => check_git_identity(repository),
        BuiltinOperation::StagedGitPolicy => check_staged_git_policy(repository),
        BuiltinOperation::LicenseHeaders => check_license_headers(repository),
        BuiltinOperation::ShellOwnership => check_shell_ownership(repository),
        BuiltinOperation::DistributionWorkflow => check_distribution_workflow(repository),
        BuiltinOperation::KernelWorkflow => check_kernel_workflow(repository),
        BuiltinOperation::PlatformWorkflow => check_platform_workflow(repository),
        BuiltinOperation::CiCache => check_ci_cache(repository),
    }
}

fn check_git_identity(repository: &Path) -> Result<(), String> {
    const EXPECTED_NAME: &str = "Nigel Breslaw";
    const EXPECTED_EMAIL: &str = "nigel.breslaw@gmail.com";
    let actual_name =
        crate::git::value(repository, &["config", "--get", "user.name"]).unwrap_or_default();
    let actual_email =
        crate::git::value(repository, &["config", "--get", "user.email"]).unwrap_or_default();
    if actual_name == EXPECTED_NAME && actual_email == EXPECTED_EMAIL {
        Ok(())
    } else {
        Err(format!(
            "git_identity_mismatch: expected {EXPECTED_NAME} <{EXPECTED_EMAIL}>; got {actual_name} <{actual_email}>"
        ))
    }
}

fn check_staged_git_policy(repository: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let paths: Vec<PathBuf> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned()))
        .collect();
    reject_forbidden_paths(repository, &paths)?;
    validate_submodules(repository, &paths)
}

fn reject_forbidden_paths(repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        let text = path.to_string_lossy();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let private_image = text.starts_with("private/")
            && matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            );
        if text.starts_with("private/test-fixtures/")
            || path
                .components()
                .any(|part| part.as_os_str() == ".wrangler")
            || name == ".env"
            || name.starts_with(".env.")
            || private_image
            || matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "7z" | "tgz" | "tar" | "gz" | "bz2" | "xz" | "rar" | "key" | "p12" | "pfx"
            )
            || matches!(name, "credentials" | "secrets" | "id_rsa" | "id_ed25519")
            || name.starts_with("credentials.")
            || name.starts_with("secrets.")
        {
            return Err(format!("staged_git_forbidden: {}", path.display()));
        }
        let ignored = Command::new("git")
            .args(["check-ignore", "-q", "--"])
            .arg(path)
            .current_dir(repository)
            .status()
            .map_err(|error| error.to_string())?;
        if ignored.success() {
            return Err(format!("staged_git_ignored: {}", path.display()));
        }
    }
    Ok(())
}

fn validate_submodules(repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        let mode = crate::git::value(
            repository,
            &["ls-files", "-s", "--", &path.to_string_lossy()],
        )?;
        if !mode.starts_with("160000 ") {
            continue;
        }
        let submodule = repository.join(path);
        if !crate::git::value(&submodule, &["status", "--porcelain"])?.is_empty() {
            return Err(format!("staged_git_dirty_submodule: {}", path.display()));
        }
        if path == Path::new("private/magik-cloud") {
            let upstream = crate::git::value(&submodule, &["rev-parse", "@{u}"])
                .map_err(|_| "staged_git_private_submodule_has_no_upstream".to_owned())?;
            let head = crate::git::value(&submodule, &["rev-parse", "HEAD"])?;
            if !crate::git::succeeds(
                &submodule,
                &["merge-base", "--is-ancestor", &head, &upstream],
            )? {
                return Err("staged_git_private_submodule_must_be_pushed_first".into());
            }
        }
    }
    Ok(())
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
            "mister/tools/host/src/platform_deploy.rs",
        ],
        &[
            "Linux-Kernel_MiSTer",
            "build-scanout-slots-module.sh",
            "coccinelle",
            "upload-artifact",
            "workflow_dispatch",
            "agent-cli/src/delivery.rs",
            "mister/tools/host/src/main.rs",
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
            "name: cargo-timings-release",
            "if-no-files-found: error",
        ],
        &[
            "target-host-",
            "Cache host build outputs",
            "scripts/agent verify --paths desktop",
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
    ] {
        if !cross.contains(&format!("\"{variable}\"")) {
            return Err(format!("distribution_contract_missing: {variable}"));
        }
    }
    for required in [
        "release_channel:",
        "scripts/agent build runtime-device",
        "scripts/agent ci require-alpha-promotion",
        "scripts/agent ci platform-manifest generate",
        "scripts/agent ci game-databases verify",
        "contents: write",
        "gh release create",
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
        "mister/tools/host/AGENTS.md",
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
        "scripts/agent check",
        "scripts/agent verify",
        "scripts/agent deliver",
        "git add --",
        "git commit -m",
        "first-attempt sandbox escalation",
    ] {
        if !root.contains(expected) {
            return Err(format!("root_workflow_missing: {expected}"));
        }
    }
    Ok(())
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
        "mister/tools/host/Cargo.toml",
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
    const RETIRED: &[&str] = &[
        "scripts/mister",
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
    }
    Ok(())
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
    fn staged_git_policy_rejects_forbidden_paths() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-staged-policy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join(".env"), "SECRET=fixture\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "-f", ".env"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            check_staged_git_policy(&root).unwrap_err(),
            "staged_git_forbidden: .env"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_identity_check_accepts_only_the_repository_identity() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-git-identity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        for (key, value) in [
            ("user.name", "Nigel Breslaw"),
            ("user.email", "nigel.breslaw@gmail.com"),
        ] {
            assert!(
                Command::new("git")
                    .args(["config", key, value])
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert!(check_git_identity(&root).is_ok());
        assert!(
            Command::new("git")
                .args(["config", "user.email", "wrong@example.invalid"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            check_git_identity(&root)
                .unwrap_err()
                .contains("git_identity_mismatch")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
