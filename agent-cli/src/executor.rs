// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{now_ms, Evidence};
use crate::model::{Operation, Outcome, Plan};
use crate::progress::{EventKind, Reporter};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
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
        reporter.emit(EventKind::Progress, "plan", "Nothing to check", Some(100))?;
        return Ok(Outcome::NoOp);
    }
    let phase = match plan.intent {
        crate::model::Intent::Verify { .. } => "verify",
        _ => "check",
    };
    for (index, operation) in plan.operations.iter().enumerate() {
        let percent = u8::try_from(index.saturating_mul(100) / plan.operations.len()).unwrap_or(0);
        let message = format!("running {}/{}", index + 1, plan.operations.len());
        reporter.emit(EventKind::Progress, phase, &message, Some(percent))?;
        let cache = operation_cache_key(evidence, repository, plan, operation)?;
        if let Some((task_id, fingerprint)) = cache.as_ref() {
            if evidence.has_cached_operation(task_id, &operation.id, fingerprint)? {
                evidence.record_reused_command(
                    request_id,
                    &operation.id,
                    &operation.program,
                    &operation.args,
                )?;
                continue;
            }
        }
        run_operation(
            evidence,
            request_id,
            repository,
            operation,
            phase,
            &message,
            &format!("{phase}: failed at {}/{}", index + 1, plan.operations.len()),
            reporter,
        )?;
        if operation.risk == crate::model::Risk::ReadOnly {
            if let Some((task_id, fingerprint)) = cache {
                evidence.cache_operation(&task_id, &operation.id, &fingerprint)?;
            }
        }
    }
    reporter.emit(
        EventKind::Completed,
        phase,
        &format!("passed — {} checks", plan.operations.len()),
        Some(100),
    )?;
    Ok(Outcome::Passed)
}

fn operation_cache_key(
    evidence: &Evidence,
    repository: &Path,
    plan: &Plan,
    operation: &Operation,
) -> Result<Option<(String, String)>, String> {
    let task_id = match &plan.intent {
        crate::model::Intent::Check {
            scope: crate::model::Scope::Task(task_id),
        }
        | crate::model::Intent::Verify {
            scope: crate::model::Scope::Task(task_id),
        } => task_id,
        _ => return Ok(None),
    };
    if !is_cacheable(operation) || operation.inputs.is_empty() {
        return Ok(None);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "planner-schema-2".hash(&mut hasher);
    operation.id.hash(&mut hasher);
    operation.program.hash(&mut hasher);
    operation.args.hash(&mut hasher);
    operation.inputs.hash(&mut hasher);
    let toolchain = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|output| output.stdout)
        .unwrap_or_default();
    toolchain.hash(&mut hasher);
    for name in [
        "RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CC",
        "CXX",
        "PKG_CONFIG_PATH",
        "MISTER_ARM_BUILD_BACKEND",
    ] {
        name.hash(&mut hasher);
        std::env::var_os(name)
            .map(|value| value.as_encoded_bytes().to_vec())
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    for path in crate::task::changes(evidence, repository, task_id)? {
        if !operation.inputs.iter().any(|input| path.starts_with(input)) {
            continue;
        }
        path.hash(&mut hasher);
        match std::fs::read(repository.join(&path)) {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "deleted".hash(&mut hasher)
            }
            Err(error) => return Err(format!("cannot fingerprint {}: {error}", path.display())),
        }
    }
    Ok(Some((task_id.clone(), format!("{:016x}", hasher.finish()))))
}

fn is_cacheable(operation: &Operation) -> bool {
    operation.risk == crate::model::Risk::ReadOnly
}

#[allow(clippy::too_many_arguments)]
fn run_operation(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    operation: &Operation,
    phase: &str,
    heartbeat: &str,
    failure_position: &str,
    reporter: &mut Reporter<'_>,
) -> Result<(), String> {
    let log_path = evidence.log_path(request_id, &operation.id);
    File::create(&log_path).map_err(|error| error.to_string())?;
    let cargo_dependency = is_cargo_dependency_operation(operation);
    let first_args = if cargo_dependency {
        cargo_args(&operation.args, true)
    } else {
        operation.args.clone()
    };
    let first_status = run_attempt(
        evidence,
        request_id,
        repository,
        operation,
        phase,
        heartbeat,
        reporter,
        &log_path,
        &first_args,
        if cargo_dependency {
            "offline"
        } else {
            "primary"
        },
    )?;
    if first_status.success() {
        return Ok(());
    }
    let first_output = read_log(&log_path)?;
    if cargo_dependency && is_offline_cache_miss(&first_output) {
        reporter.emit(
            EventKind::Warning,
            operation_phase(operation),
            &format!(
                "dependency_cache_missing: {} — retrying locked dependencies with network",
                operation.title
            ),
            None,
        )?;
        let online_args = cargo_args(&operation.args, false);
        let online_start = std::fs::metadata(&log_path)
            .map_err(|error| error.to_string())?
            .len();
        let online_status = run_attempt(
            evidence,
            request_id,
            repository,
            operation,
            phase,
            heartbeat,
            reporter,
            &log_path,
            &online_args,
            "network-fallback",
        )?;
        if online_status.success() {
            return Ok(());
        }
        let online_output = read_log_from(&log_path, online_start)?;
        return Err(failure_message(
            evidence,
            operation,
            request_id,
            &log_path,
            online_status.code().unwrap_or(1),
            &online_output,
            failure_position,
        )?);
    }
    Err(failure_message(
        evidence,
        operation,
        request_id,
        &log_path,
        first_status.code().unwrap_or(1),
        &first_output,
        failure_position,
    )?)
}

#[allow(clippy::too_many_arguments)]
fn run_attempt(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    operation: &Operation,
    phase: &str,
    heartbeat: &str,
    reporter: &mut Reporter<'_>,
    log_path: &Path,
    args: &[String],
    attempt: &str,
) -> Result<std::process::ExitStatus, String> {
    let mut log = OpenOptions::new()
        .append(true)
        .open(log_path)
        .map_err(|error| error.to_string())?;
    writeln!(log, "=== agent-cli attempt: {attempt} ===").map_err(|error| error.to_string())?;
    let started = now_ms();
    let command_id = evidence.begin_command(
        request_id,
        &operation.id,
        &operation.program,
        args,
        Some(log_path),
    )?;
    let mut command = Command::new(&operation.program);
    command.args(args).current_dir(repository);
    if attempt == "network-fallback" {
        command.env("CARGO_NET_RETRY", "0");
    }
    let mut child = command
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
        reporter.emit(EventKind::Progress, phase, heartbeat, None)?;
    };
    let code = status.code().unwrap_or(1);
    evidence.finish_command(command_id, started, code)?;
    Ok(status)
}

fn is_cargo_dependency_operation(operation: &Operation) -> bool {
    Path::new(&operation.program).file_name() == Some(OsStr::new("cargo"))
        && operation.args.first().is_some_and(|arg| arg != "fmt")
}

fn cargo_args(args: &[String], offline: bool) -> Vec<String> {
    let mut result: Vec<_> = args
        .iter()
        .filter(|arg| arg.as_str() != "--locked" && arg.as_str() != "--offline")
        .cloned()
        .collect();
    let separator = result
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(result.len());
    result.insert(separator, "--locked".into());
    if offline {
        result.insert(separator + 1, "--offline".into());
    }
    result
}

fn is_offline_cache_miss(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("--offline was specified")
        || lower.contains("no matching package named") && lower.contains("offline mode")
        || lower.contains("failed to download") && lower.contains("offline")
}

fn failure_classification(operation: &Operation, code: i32, output: &str) -> &'static str {
    let lower = output.to_ascii_lowercase();
    if code == 127 {
        "command_missing"
    } else if is_dependency_fetch_failure(&lower) && is_network_unavailable(&lower) {
        "network_required"
    } else if is_dependency_fetch_failure(&lower) {
        "dependency_fetch_failed"
    } else if operation.args.first().is_some_and(|arg| arg == "test")
        && (lower.contains("test result: failed")
            || lower.contains("test failed")
            || lower.contains("failures:"))
    {
        "test_failure"
    } else {
        "command_failed"
    }
}

fn is_network_unavailable(lower: &str) -> bool {
    [
        "couldn't resolve host",
        "could not resolve host",
        "failed to resolve host",
        "could not resolve proxy",
        "temporary failure in name resolution",
        "name or service not known",
        "network is unreachable",
        "connection refused",
        "connection reset by peer",
        "could not connect to server",
        "failed to connect",
        "operation timed out",
        "proxy connect aborted",
        "operation not permitted",
        "network permission denied",
        "socket access is forbidden",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn is_dependency_fetch_failure(lower: &str) -> bool {
    lower.contains("unable to update registry")
        || lower.contains("failed to download")
        || lower.contains("download of config.json failed")
        || lower.contains("failed to get") && lower.contains("as a dependency")
        || lower.contains("failed to authenticate")
        || lower.contains("authentication required")
        || lower.contains("credential-provider")
        || lower.contains("http 401")
        || lower.contains("http 403")
}

fn failure_message(
    evidence: &Evidence,
    operation: &Operation,
    request_id: &str,
    log_path: &Path,
    code: i32,
    output: &str,
    failure_position: &str,
) -> Result<String, String> {
    let classification = failure_classification(operation, code, output);
    let next = if classification == "network_required" {
        format!(
            "rerun with network access: {}",
            retry_command(&evidence.request_args(request_id)?)
        )
    } else {
        format!("scripts/agent run show {request_id}")
    };
    Ok(format!(
        "{failure_position} — {}\nerror: {classification} (exit {code})\nsummary: {}\nlog: {}\nnext: {next}",
        operation.title,
        log_tail(log_path)?,
        log_path.display()
    ))
}

fn retry_command(args: &[String]) -> String {
    std::iter::once("scripts/agent".to_owned())
        .chain(args.iter().skip(1).map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./".contains(&byte))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn read_log(path: &Path) -> Result<String, String> {
    read_log_from(path, 0)
}

fn read_log_from(path: &Path, start: u64) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(start))
        .map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(text)
}

fn operation_phase(operation: &Operation) -> &'static str {
    if operation.id.starts_with("arm.") {
        "arm-build"
    } else if operation.id.starts_with("release.") {
        "release"
    } else {
        "check"
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
        .filter(|line| !line.contains("to rerun pass") && !line.contains("cargo test"))
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
    use crate::cli::OutputFormat;
    use crate::model::{Intent, Risk, Scope};
    use crate::request::RawRequest;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_NONCE: AtomicU64 = AtomicU64::new(0);

    fn test_operation(program: &Path) -> Operation {
        Operation {
            id: "test.cargo".into(),
            title: "Test fake crate".into(),
            risk: Risk::ReadOnly,
            program: program.display().to_string(),
            args: vec!["test".into(), "--".into(), "--nocapture".into()],
            reason: "executor test".into(),
            failure_hint: "inspect run".into(),
            inputs: vec!["fixture".into()],
        }
    }

    fn fake_cargo(script: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = TEST_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "agent-cli-cargo-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let cargo = root.join("cargo");
        fs::write(&cargo, script).unwrap();
        let mut permissions = fs::metadata(&cargo).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cargo, permissions).unwrap();
        (root, cargo)
    }

    fn execute_fake_cargo(
        script: &str,
    ) -> (Result<Outcome, String>, crate::evidence::RunDetail, String) {
        let (root, cargo) = fake_cargo(script);
        let state = root.join("state");
        let evidence = Evidence::open_at(&state).unwrap();
        let request = RawRequest {
            id: "test-run".into(),
            args: vec!["agent-cli".into(), "check".into()],
        };
        evidence.begin_request(&request).unwrap();
        evidence
            .record_intent(
                &request.id,
                &Intent::Check {
                    scope: Scope::WorkingTree,
                },
            )
            .unwrap();
        let plan = Plan {
            intent: Intent::Check {
                scope: Scope::WorkingTree,
            },
            operations: vec![test_operation(&cargo)],
            external_requirements: Vec::new(),
        };
        let mut reporter = Reporter::new(&evidence, OutputFormat::Human, &request.id);
        let result = execute(&evidence, &request.id, &root, &plan, &mut reporter);
        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        let log = fs::read_to_string(detail.commands[0].log_path.as_ref().unwrap()).unwrap();
        fs::remove_dir_all(root).unwrap();
        (result, detail, log)
    }

    #[test]
    fn cargo_flags_are_locked_offline_and_precede_separator() {
        let args = vec!["test".into(), "--".into(), "--nocapture".into()];
        assert_eq!(
            cargo_args(&args, true),
            ["test", "--locked", "--offline", "--", "--nocapture"]
        );
        assert_eq!(
            cargo_args(&args, false),
            ["test", "--locked", "--", "--nocapture"]
        );
    }

    #[test]
    fn cargo_format_is_excluded_from_dependency_policy() {
        let mut operation = test_operation(Path::new("cargo"));
        operation.args = vec!["fmt".into(), "--check".into()];
        assert!(!is_cargo_dependency_operation(&operation));
        assert_eq!(operation.args, ["fmt", "--check"]);
    }

    #[test]
    fn cached_cargo_success_records_one_offline_attempt() {
        let (result, detail, log) = execute_fake_cargo("#!/bin/sh\nexit 0\n");
        assert_eq!(result.unwrap(), Outcome::Passed);
        assert_eq!(detail.commands.len(), 1);
        assert!(log.contains("=== agent-cli attempt: offline ==="));
        assert!(detail.commands[0]
            .args
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--offline"));
    }

    #[test]
    fn offline_cache_miss_retries_online_and_records_both_attempts() {
        let script = "#!/bin/sh\ncase \" $* \" in\n  *\" --offline \"*) echo 'error: failed to download crate: attempting to make an HTTP request, but --offline was specified' >&2; exit 101;;\n  *) exit 0;;\nesac\n";
        let (result, detail, log) = execute_fake_cargo(script);
        assert_eq!(result.unwrap(), Outcome::Passed);
        assert_eq!(detail.commands.len(), 2);
        assert!(detail.commands[0].args.to_string().contains("--offline"));
        assert!(!detail.commands[1].args.to_string().contains("--offline"));
        assert!(detail.commands[1].args.to_string().contains("--locked"));
        assert!(log.contains("=== agent-cli attempt: offline ==="));
        assert!(log.contains("=== agent-cli attempt: network-fallback ==="));
    }

    #[test]
    fn network_failure_after_cache_miss_is_not_a_test_failure() {
        let script = "#!/bin/sh\ncase \" $* \" in\n  *\" --offline \"*) echo 'attempting to make an HTTP request, but --offline was specified' >&2;;\n  *) echo \"unable to update registry crates-io: Couldn't resolve host: index.crates.io\" >&2;;\nesac\nexit 101\n";
        let (result, detail, _) = execute_fake_cargo(script);
        let error = result.unwrap_err();
        assert!(error.contains("error: network_required"));
        assert!(error.contains("rerun with network access: scripts/agent check"));
        assert_eq!(detail.commands.len(), 2);
    }

    #[test]
    fn genuine_test_failure_does_not_retry() {
        let (result, detail, _) = execute_fake_cargo(
            "#!/bin/sh\necho 'test result: FAILED. 0 passed; 1 failed' >&2\nexit 101\n",
        );
        assert!(result.unwrap_err().contains("error: test_failure"));
        assert_eq!(detail.commands.len(), 1);
    }

    #[test]
    fn compiler_and_stale_lock_failures_do_not_retry() {
        let operation = test_operation(Path::new("cargo"));
        assert_eq!(
            failure_classification(&operation, 101, "error[E0308]: mismatched types"),
            "command_failed"
        );
        assert!(!is_offline_cache_miss(
            "the lock file needs to be updated but --locked was passed"
        ));
    }

    #[test]
    fn only_read_only_operations_are_cacheable() {
        let mut operation = test_operation(Path::new("cargo"));
        assert!(is_cacheable(&operation));
        operation.risk = Risk::LocalWrite;
        assert!(!is_cacheable(&operation));
        operation.risk = Risk::DeviceWrite;
        assert!(!is_cacheable(&operation));
    }

    #[test]
    fn network_text_from_an_executed_test_remains_a_test_failure() {
        let operation = test_operation(Path::new("cargo"));
        assert_eq!(
            failure_classification(
                &operation,
                101,
                "test service_connect ... FAILED\nconnection refused\ntest result: FAILED"
            ),
            "test_failure"
        );
    }

    #[test]
    fn registry_authentication_failure_is_a_dependency_fetch_failure() {
        let operation = test_operation(Path::new("cargo"));
        assert_eq!(
            failure_classification(
                &operation,
                101,
                "failed to get package as a dependency: failed to authenticate; credential-provider missing"
            ),
            "dependency_fetch_failed"
        );
    }

    #[test]
    fn common_network_and_sandbox_denials_require_network_access() {
        let operation = test_operation(Path::new("cargo"));
        for message in [
            "Could not resolve proxy",
            "Temporary failure in name resolution",
            "Name or service not known",
            "Connection reset by peer",
            "Operation not permitted",
            "network permission denied",
            "socket access is forbidden",
        ] {
            let output = format!("unable to update registry crates-io: {message}");
            assert_eq!(
                failure_classification(&operation, 101, &output),
                "network_required",
                "message: {message}"
            );
        }
    }
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

    #[test]
    fn log_tail_suppresses_raw_cargo_retry_advice() {
        let path = std::env::temp_dir().join(format!("agent-cli-retry-{}", std::process::id()));
        fs::write(
            &path,
            "test failed\nerror: test failed, to rerun pass `cargo test --lib`\nuse the harness\n",
        )
        .unwrap();
        let tail = log_tail(&path).unwrap();
        assert_eq!(tail, "test failed | use the harness");
        fs::remove_file(path).unwrap();
    }
}
