// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;
use std::process::Command;

use crate::error::AgentResult;

/// Preserve the library interface while Python owns metadata policy.
pub fn report(repository: &Path, requested: &Path) -> AgentResult<String> {
    report_with_format(repository, requested, false)
}

pub fn report_with_format(repository: &Path, requested: &Path, json: bool) -> AgentResult<String> {
    let mut command = Command::new("python3");
    command
        .arg(repository.join("scripts/magik_ci/guidance.py"))
        .arg("--repository")
        .arg(repository);
    if json {
        command.arg("--json");
    }
    let output = command
        .arg("--")
        .arg(requested)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string().into())
}
