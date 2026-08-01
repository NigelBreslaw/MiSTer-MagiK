// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<Outcome> {
    let mut failures = Vec::new();
    reporter.emit(
        EventKind::Progress,
        "doctor",
        "Checking host commands",
        Some(10),
    )?;
    for command in ["git", "cargo", "rustup", "node", "corepack"] {
        require_command(command, &mut failures);
    }

    reporter.emit(
        EventKind::Progress,
        "doctor",
        "Checking Rust tooling",
        Some(30),
    )?;
    check_rust(repository, reporter, &mut failures)?;

    reporter.emit(
        EventKind::Progress,
        "doctor",
        "Checking desktop and docs",
        Some(60),
    )?;
    check_desktop(repository, &mut failures);
    check_docs(repository, reporter, &mut failures)?;

    reporter.emit(
        EventKind::Progress,
        "doctor",
        "Checking repository outputs",
        Some(85),
    )?;
    check_outputs(repository, reporter, &mut failures)?;
    let entrypoint = repository.join("scripts/agent");
    if !is_executable(&entrypoint) {
        failures.push("scripts/agent is missing or not executable".into());
    }

    if failures.is_empty() {
        reporter.emit(EventKind::Completed, "doctor", "Host is ready", Some(100))?;
        Ok(Outcome::Passed)
    } else {
        Err(AgentError::Classified {
            code: "host_not_ready",
            detail: failures.join("; "),
        })
    }
}

fn require_command(name: &str, failures: &mut Vec<String>) {
    if find_command(name).is_none() {
        failures.push(format!("{name} is not installed or not on PATH"));
    }
}

fn find_command(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .as_encoded_bytes()
        .split(|byte| *byte == b':')
        .find_map(|directory| {
            let path = Path::new(std::ffi::OsStr::from_bytes(directory)).join(name);
            is_executable(&path).then_some(path)
        })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn check_rust(
    repository: &Path,
    reporter: &mut Reporter<'_>,
    failures: &mut Vec<String>,
) -> AgentResult<()> {
    let toolchain = fs::read_to_string(repository.join("apps/mister/rust-toolchain.toml"))
        .map_err(|error| format!("cannot read pinned Rust toolchain: {error}"))?;
    let channel = quoted_value(&toolchain, "channel")
        .ok_or("apps/mister/rust-toolchain.toml has no channel")?;
    let installed = output(repository, reporter, "rustup", &["toolchain", "list"])?;
    if !installed.lines().any(|line| line.starts_with(&channel)) {
        failures.push(format!("Rust toolchain {channel} is not installed"));
    }
    let components = output(
        repository,
        reporter,
        "rustup",
        &["component", "list", "--toolchain", &channel],
    )?;
    for component in ["rustfmt", "clippy"] {
        if !components
            .lines()
            .any(|line| line.starts_with(component) && line.ends_with("(installed)"))
        {
            failures.push(format!(
                "Rust component {component} is missing for {channel}"
            ));
        }
    }
    Ok(())
}

fn check_desktop(repository: &Path, failures: &mut Vec<String>) {
    for name in ["github-app", "material-icon-theme"] {
        if !repository
            .join("apps/desktop/vendor")
            .join(name)
            .join(".git")
            .exists()
        {
            failures.push(format!("desktop submodule {name} is not initialized"));
        }
    }
}

fn check_docs(
    repository: &Path,
    reporter: &mut Reporter<'_>,
    failures: &mut Vec<String>,
) -> AgentResult<()> {
    let node = output(repository, reporter, "node", &["--version"])?;
    let major = node
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|v| v.parse::<u32>().ok());
    if major.is_none_or(|major| major < 22) {
        failures.push("Node.js 22 or newer is required".into());
    }
    let pnpm = output(repository, reporter, "corepack", &["pnpm", "--version"])?;
    if pnpm.trim() != "11.10.0" {
        failures.push("Corepack pnpm 11.10.0 is required".into());
    }
    let modules = repository.join("documentation/node_modules");
    if !modules.join(".pnpm").is_dir() || !is_executable(&modules.join(".bin/astro")) {
        failures.push("documentation dependencies are not installed".into());
    }
    Ok(())
}

fn check_outputs(
    repository: &Path,
    reporter: &mut Reporter<'_>,
    failures: &mut Vec<String>,
) -> AgentResult<()> {
    for output_path in ["build", "dist", "outputs", "target", "documentation/dist"] {
        let sentinel = format!("{output_path}/.mister-magik-doctor");
        let status = command_status(
            repository,
            reporter,
            "git",
            &["check-ignore", "-q", &sentinel],
        )?;
        if !status {
            failures.push(format!("{output_path} is not ignored by Git"));
        }
    }
    Ok(())
}

fn output(
    repository: &Path,
    reporter: &mut Reporter<'_>,
    program: &str,
    args: &[&str],
) -> AgentResult<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(repository)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| AgentError::Classified {
            code: "command_missing",
            detail: format!("cannot start {program}: {error}"),
        })?;
    let status = crate::process::wait(
        &mut child,
        Some(Duration::from_secs(30)),
        program,
        Some(Duration::from_secs(10)),
        || {
            reporter.emit(
                EventKind::Progress,
                "doctor",
                "Inspecting host prerequisites",
                None,
            )
        },
    )?;
    let mut text = String::new();
    child
        .stdout
        .take()
        .ok_or("command stdout unavailable")?
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read {program} output: {error}"))?;
    if status.success() {
        Ok(text.trim().to_owned())
    } else {
        Err(AgentError::Classified {
            code: "host_probe_failed",
            detail: format!("{program} {} exited with {status}", args.join(" ")),
        })
    }
}

fn command_status(
    repository: &Path,
    reporter: &mut Reporter<'_>,
    program: &str,
    args: &[&str],
) -> AgentResult<bool> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(repository)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    Ok(crate::process::wait(
        &mut child,
        Some(Duration::from_secs(5)),
        program,
        Some(Duration::from_secs(10)),
        || {
            reporter.emit(
                EventKind::Progress,
                "doctor",
                "Inspecting host prerequisites",
                None,
            )
        },
    )?
    .success())
}

fn quoted_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim();
        value
            .strip_prefix('"')?
            .strip_suffix('"')
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_pinned_channel() {
        assert_eq!(
            quoted_value("[toolchain]\nchannel = \"1.97.0\"\n", "channel").as_deref(),
            Some("1.97.0")
        );
    }

    #[test]
    fn rejects_unquoted_channel() {
        assert_eq!(quoted_value("channel = stable", "channel"), None);
    }

    #[test]
    fn quoted_values_require_exact_keys_and_balanced_quotes() {
        let text = "channel_name = \"wrong\"\n channel = \"1.97.1\"\nprofile = \"minimal\"\n";
        assert_eq!(quoted_value(text, "channel").as_deref(), Some("1.97.1"));
        assert_eq!(quoted_value(text, "profile").as_deref(), Some("minimal"));
        assert_eq!(quoted_value("channel = \"unterminated", "channel"), None);
        assert_eq!(quoted_value(text, "missing"), None);
    }

    #[test]
    fn executable_and_desktop_prerequisites_are_reported_from_fixtures() {
        let root = std::env::temp_dir().join(format!("agent-doctor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let command = root.join("tool");
        fs::write(&command, b"#!/bin/sh\n").unwrap();
        assert!(!is_executable(&command));
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();
        assert!(is_executable(&command));

        let mut failures = Vec::new();
        check_desktop(&root, &mut failures);
        assert_eq!(failures.len(), 2);
        for name in ["github-app", "material-icon-theme"] {
            fs::create_dir_all(root.join("apps/desktop/vendor").join(name).join(".git")).unwrap();
        }
        failures.clear();
        check_desktop(&root, &mut failures);
        assert!(failures.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
