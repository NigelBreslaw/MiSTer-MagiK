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
    let label = match operation {
        BuiltinOperation::AgentGuidance => "agent guidance",
        BuiltinOperation::LicenseHeaders => "license headers",
        BuiltinOperation::ShellOwnership => "shell ownership",
        BuiltinOperation::DistributionWorkflow => "distribution workflow",
    };
    reporter.emit(
        EventKind::Progress,
        "check",
        &format!("Checking {label}"),
        None,
    )?;
    match operation {
        BuiltinOperation::AgentGuidance => check_agent_guidance(repository),
        BuiltinOperation::LicenseHeaders => check_license_headers(repository),
        BuiltinOperation::ShellOwnership => check_shell_ownership(repository),
        BuiltinOperation::DistributionWorkflow => check_distribution_workflow(repository),
    }
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
        "game-databases-bundle.py verify",
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
        "game-databases-bundle.py create",
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
    for command in ["plan", "check", "verify", "commit", "deliver"] {
        let expected = format!("scripts/agent {command}");
        if !root.contains(&expected) {
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
    let mut files = Vec::new();
    for root in [
        "AGENTS.md",
        "agent-cli",
        "apps",
        "docs",
        "documentation",
        "mister",
        "scripts",
        ".github",
    ] {
        collect_files(&repository.join(root), &mut files)?;
    }
    let exclusions: BTreeSet<&str> = [
        "agent-cli/src/checks.rs",
        "docs/agents/script-deletion-ledger.md",
    ]
    .into_iter()
    .collect();
    for path in files {
        let relative = path
            .strip_prefix(repository)
            .unwrap_or(&path)
            .to_string_lossy();
        if exclusions.contains(relative.as_ref())
            || relative.starts_with("docs/performance-review-")
            || relative.starts_with("docs/2026-")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
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

fn collect_files(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        output.push(path.to_owned());
        return Ok(());
    }
    for entry in
        fs::read_dir(path).map_err(|error| format!("cannot scan {}: {error}", path.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir()
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("target" | "node_modules" | "build" | "dist")
            )
        {
            continue;
        }
        collect_files(&path, output)?;
    }
    Ok(())
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
}
