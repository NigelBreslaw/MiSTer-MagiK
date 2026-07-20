// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{now_ms, Evidence};
use crate::model::{Operation, Outcome, Plan};
use crate::progress::{EventKind, Reporter};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub fn execute(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    plan: &Plan,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    if plan.operations.is_empty() {
        reporter.emit(EventKind::Progress, "plan", "Nothing to lint", Some(100))?;
        return Ok(Outcome::NoOp);
    }
    for (index, operation) in plan.operations.iter().enumerate() {
        let percent = u8::try_from(index.saturating_mul(100) / plan.operations.len()).unwrap_or(0);
        reporter.emit(
            EventKind::Progress,
            operation_phase(operation),
            &operation.title,
            Some(percent),
        )?;
        run_operation(evidence, request_id, repository, operation, reporter)?;
    }
    Ok(Outcome::Passed)
}

fn run_operation(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    operation: &Operation,
    reporter: &mut Reporter<'_>,
) -> Result<(), String> {
    let log_path = evidence.log_path(request_id, &operation.id);
    let log = File::create(&log_path).map_err(|error| error.to_string())?;
    let started = now_ms();
    let command_id = evidence.begin_command(
        request_id,
        &operation.id,
        &operation.program,
        &operation.args,
        Some(&log_path),
    )?;
    let mut child = Command::new(&operation.program)
        .args(&operation.args)
        .current_dir(repository)
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| error.to_string())?;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        thread::sleep(Duration::from_millis(100));
        reporter.emit(
            EventKind::Progress,
            operation_phase(operation),
            &operation.title,
            None,
        )?;
    };
    let code = status.code().unwrap_or(1);
    evidence.finish_command(command_id, started, code)?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "{} failed (exit {code}); log={} tail={}",
        operation.title,
        log_path.display(),
        log_tail(&log_path)?
    ))
}

fn operation_phase(operation: &Operation) -> &'static str {
    if operation.id.starts_with("arm.") {
        "arm-build"
    } else if operation.id.starts_with("release.") {
        "release"
    } else {
        "lint"
    }
}

fn log_tail(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(4_096)))
        .map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text
        .lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn log_tail_is_bounded_to_eight_lines() {
        let path = std::env::temp_dir().join(format!("agent-cli-tail-{}", std::process::id()));
        fs::write(
            &path,
            (0..12)
                .map(|line| format!("line {line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        assert_eq!(log_tail(&path).unwrap().split(" | ").count(), 8);
        fs::remove_file(path).unwrap();
    }
}
