// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{now_ms, Evidence};
use crate::model::{Operation, Outcome, Plan};
use crate::progress::{EventKind, Reporter};
use crate::workflow::{Event, Machine, State};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn execute(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    plan: &Plan,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    let changes = match &plan.intent {
        crate::model::Intent::Check {
            scope: crate::model::Scope::Task(task_id),
        }
        | crate::model::Intent::Verify {
            scope: crate::model::Scope::Task(task_id),
        } => crate::task::changes(evidence, repository, task_id)?,
        _ => Vec::new(),
    };
    execute_with_changes(evidence, request_id, repository, plan, &changes, reporter)
}

pub fn execute_with_changes(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    plan: &Plan,
    changes: &[PathBuf],
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    if plan.operations.is_empty() {
        reporter.emit(EventKind::Progress, "plan", "Nothing to check", Some(100))?;
        return Ok(Outcome::NoOp);
    }
    let command = match plan.intent {
        crate::model::Intent::Verify { .. } => "verify",
        _ => "check",
    };
    let fingerprints = FingerprintContext::new(repository, &plan.operations, changes)?;
    let mut machine = Machine::default();
    let mut index = 0;
    while index < plan.operations.len() {
        let operation = &plan.operations[index];
        crate::policy::authorize(operation, plan.intent.risk()).map_err(|rejection| {
            format!(
                "policy_rejected: {}: {}",
                rejection.operation_id, rejection.reason
            )
        })?;
        let state = State::from(operation.workflow_phase());
        if machine.state() != state {
            machine.apply(Event::Advance(state))?;
            reporter.emit(EventKind::Progress, command, state.label(), None)?;
        }
        if operation.builtin.is_some() && operation.risk == crate::model::Risk::ReadOnly {
            let start = index;
            while index < plan.operations.len()
                && plan.operations[index].workflow_phase() == operation.workflow_phase()
                && plan.operations[index].builtin.is_some()
                && plan.operations[index].risk == crate::model::Risk::ReadOnly
            {
                crate::policy::authorize(&plan.operations[index], plan.intent.risk()).map_err(
                    |rejection| {
                        format!(
                            "policy_rejected: {}: {}",
                            rejection.operation_id, rejection.reason
                        )
                    },
                )?;
                index += 1;
            }
            if let Err(error) = run_builtin_batch(
                evidence,
                request_id,
                repository,
                plan,
                &plan.operations[start..index],
                &fingerprints,
                reporter,
                command,
            ) {
                machine.apply(Event::Fail)?;
                return Err(error);
            }
            continue;
        }
        let heartbeat = operation_heartbeat(operation);
        let cache = operation_cache_key(plan, operation, &fingerprints)?;
        if let Some(fingerprint) = cache.as_ref() {
            if let Some((result, detail)) =
                evidence.cached_validation(&operation.id, fingerprint)?
            {
                evidence.record_reused_command(
                    request_id,
                    &operation.id,
                    &operation.program,
                    &operation.args,
                )?;
                if result == "failed" {
                    machine.apply(Event::Fail)?;
                    return Err(detail.unwrap_or_else(|| "cached validation failed".into()));
                }
                index += 1;
                continue;
            }
        }
        if let Some(fingerprint) = cache.as_ref() {
            if !evidence.claim_validation(&operation.id, fingerprint, request_id)? {
                let joined_owner = evidence.validation_owner(&operation.id, fingerprint)?;
                let cached =
                    wait_for_validation(evidence, reporter, command, operation, fingerprint)?;
                if let Some(owner) = joined_owner.as_deref() {
                    evidence.record_joined_command(
                        request_id,
                        owner,
                        &operation.id,
                        &operation.program,
                        &operation.args,
                    )?;
                }
                if let Some((result, detail)) = cached {
                    if result == "failed" {
                        machine.apply(Event::Fail)?;
                        return Err(detail.unwrap_or_else(|| "joined validation failed".into()));
                    }
                    index += 1;
                    continue;
                }
            }
        }
        if let Err(error) = run_operation(
            evidence,
            request_id,
            repository,
            operation,
            command,
            heartbeat,
            &format!("{command}: failed"),
            reporter,
        ) {
            if let Some(fingerprint) = cache.as_ref() {
                if deterministic_failure(&error) {
                    evidence.cache_validation(
                        &operation.id,
                        fingerprint,
                        "failed",
                        Some(&error),
                    )?;
                }
                evidence.release_validation(&operation.id, fingerprint, request_id)?;
            }
            machine.apply(Event::Fail)?;
            return Err(error);
        }
        if operation.risk == crate::model::Risk::ReadOnly {
            if let Some(fingerprint) = cache {
                evidence.cache_validation(&operation.id, &fingerprint, "passed", None)?;
                evidence.release_validation(&operation.id, &fingerprint, request_id)?;
            }
        }
        index += 1;
    }
    machine.apply(Event::Finish)?;
    reporter.emit(EventKind::Completed, command, "passed", Some(100))?;
    Ok(Outcome::Passed)
}

fn wait_for_validation(
    evidence: &Evidence,
    reporter: &mut Reporter<'_>,
    phase: &str,
    operation: &Operation,
    fingerprint: &str,
) -> Result<Option<(String, Option<String>)>, String> {
    let started = Instant::now();
    let mut next_progress = Duration::from_secs(10);
    while started.elapsed() < Duration::from_secs(31 * 60) {
        if let Some(result) = evidence.cached_validation(&operation.id, fingerprint)? {
            return Ok(Some(result));
        }
        if evidence
            .validation_owner(&operation.id, fingerprint)?
            .is_none()
        {
            return Ok(None);
        }
        if started.elapsed() >= next_progress {
            reporter.emit(
                EventKind::Progress,
                phase,
                &format!("Waiting for shared {}", operation.title),
                None,
            )?;
            next_progress += Duration::from_secs(10);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "validation_wait_timeout: shared {} exceeded its deadline",
        operation.title
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_builtin_batch(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    plan: &Plan,
    operations: &[Operation],
    fingerprints: &FingerprintContext,
    reporter: &mut Reporter<'_>,
    command: &str,
) -> Result<(), String> {
    let mut pending = Vec::new();
    for operation in operations {
        let cache = operation_cache_key(plan, operation, fingerprints)?;
        if let Some(fingerprint) = cache.as_ref() {
            if evidence
                .cached_validation(&operation.id, fingerprint)?
                .is_some_and(|(result, _)| result == "passed")
            {
                evidence.record_reused_command(
                    request_id,
                    &operation.id,
                    &operation.program,
                    &operation.args,
                )?;
                continue;
            }
        }
        let builtin = operation.builtin.expect("batch contains only builtins");
        reporter.emit(
            EventKind::Progress,
            command,
            &format!("Checking {}", crate::checks::label(builtin)),
            None,
        )?;
        pending.push((operation, builtin, cache));
    }
    let limit = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(4);
    for chunk in pending.chunks(limit) {
        let results = run_parallel_ordered(chunk, limit, |(_, builtin, _)| {
            crate::checks::run(*builtin, repository)
        });
        let mut first_error = None;
        for ((operation, _, cache), result) in chunk.iter().zip(results) {
            if let Err(error) = result {
                first_error.get_or_insert_with(|| {
                    format!("{command}: failed — {}\nerror: {error}", operation.title)
                });
            } else if let Some(fingerprint) = cache {
                evidence.cache_validation(&operation.id, fingerprint, "passed", None)?;
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    Ok(())
}

fn run_parallel_ordered<T: Sync>(
    items: &[T],
    limit: usize,
    run: impl Fn(&T) -> Result<(), String> + Sync,
) -> Vec<Result<(), String>> {
    let mut results = Vec::with_capacity(items.len());
    for chunk in items.chunks(limit.max(1)) {
        results.extend(std::thread::scope(|scope| {
            chunk
                .iter()
                .map(|item| scope.spawn(|| run(item)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .unwrap_or_else(|_| Err("validation worker panicked".into()))
                })
                .collect::<Vec<_>>()
        }));
    }
    results
}

fn operation_cache_key(
    plan: &Plan,
    operation: &Operation,
    fingerprints: &FingerprintContext,
) -> Result<Option<String>, String> {
    match &plan.intent {
        crate::model::Intent::Check {
            scope: crate::model::Scope::Task(_),
        }
        | crate::model::Intent::Verify {
            scope: crate::model::Scope::Task(_),
        } => {}
        _ => return Ok(None),
    }
    if !is_cacheable(operation) || operation.inputs.is_empty() {
        return Ok(None);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "planner-schema-2".hash(&mut hasher);
    operation.id.hash(&mut hasher);
    operation.program.hash(&mut hasher);
    operation.args.hash(&mut hasher);
    operation.inputs.hash(&mut hasher);
    fingerprints.toolchain.hash(&mut hasher);
    fingerprints.environment.hash(&mut hasher);
    for (path, fingerprint) in &fingerprints.files {
        if !operation.inputs.iter().any(|input| path.starts_with(input)) {
            continue;
        }
        path.hash(&mut hasher);
        fingerprint.hash(&mut hasher);
    }
    Ok(Some(format!("{:016x}", hasher.finish())))
}

fn deterministic_failure(error: &str) -> bool {
    (error.contains("test_failure") || error.contains("command_failed"))
        && ![
            "timed out",
            "permission",
            "network",
            "cancelled",
            "container",
            "index lock",
        ]
        .iter()
        .any(|value| error.to_ascii_lowercase().contains(value))
}

#[derive(Debug)]
struct FingerprintContext {
    toolchain: Vec<u8>,
    environment: Vec<(String, Vec<u8>)>,
    files: BTreeMap<PathBuf, String>,
}

impl FingerprintContext {
    fn new(
        repository: &Path,
        operations: &[Operation],
        changes: &[PathBuf],
    ) -> Result<Self, String> {
        let toolchain = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .map(|output| output.stdout)
            .unwrap_or_default();
        let environment = [
            "RUSTFLAGS",
            "CARGO_BUILD_TARGET",
            "CC",
            "CXX",
            "PKG_CONFIG_PATH",
            "MISTER_ARM_BUILD_BACKEND",
        ]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                std::env::var_os(name)
                    .map(|value| value.as_encoded_bytes().to_vec())
                    .unwrap_or_default(),
            )
        })
        .collect();
        let mut files = BTreeMap::new();
        for path in changes {
            let relevant = operations.iter().any(|operation| {
                is_cacheable(operation)
                    && operation.inputs.iter().any(|input| path.starts_with(input))
            });
            if relevant {
                files.insert(path.clone(), fingerprint_path(&repository.join(path))?);
            }
        }
        Ok(Self {
            toolchain,
            environment,
            files,
        })
    }
}

fn fingerprint_path(path: &Path) -> Result<String, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("deleted".into()),
        Err(error) => return Err(format!("cannot fingerprint {}: {error}", path.display())),
    };
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    let mode = metadata.permissions().mode();
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    let (kind, digest) = if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        (
            "symlink",
            Sha256::digest(target.as_os_str().as_encoded_bytes()).to_vec(),
        )
    } else if metadata.is_file() {
        let mut file = File::open(path)
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        ("file", hasher.finalize().to_vec())
    } else if metadata.is_dir() && path.join(".git").exists() {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        if !output.status.success() {
            return Err(format!("cannot fingerprint submodule {}", path.display()));
        }
        ("gitlink", output.stdout)
    } else {
        ("other", Vec::new())
    };
    Ok(format!("{kind}:{mode:o}:{}", hex(&digest)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn is_cacheable(operation: &Operation) -> bool {
    operation.risk == crate::model::Risk::ReadOnly
}

fn operation_heartbeat(operation: &Operation) -> &str {
    &operation.title
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
    if let Some(builtin) = operation.builtin {
        return crate::checks::execute(builtin, repository, reporter)
            .map_err(|error| format!("{failure_position} — {}\nerror: {error}", operation.title));
    }
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
    let status = crate::process::wait(
        &mut child,
        None,
        &operation.program,
        Some(std::time::Duration::from_millis(
            crate::progress::HEARTBEAT_MS,
        )),
        || {
            reporter
                .emit(EventKind::Progress, phase, heartbeat, None)
                .map_err(|error| error.to_string())
        },
    )?;
    let code = status.code().unwrap_or(1);
    evidence.finish_command(command_id, started, code)?;
    Ok(status)
}

fn is_cargo_dependency_operation(operation: &Operation) -> bool {
    operation.cargo_offline_first()
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
            crate::shell::agent_retry_command(&evidence.request_args(request_id)?)
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
    use crate::model::{ActionKind, Intent, Risk, Scope, WorkflowPhase};
    use crate::request::RawRequest;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    static TEST_NONCE: AtomicU64 = AtomicU64::new(0);

    fn test_operation(program: &Path) -> Operation {
        Operation {
            id: "test.cargo".into(),
            title: "Test fake crate".into(),
            risk: Risk::ReadOnly,
            action: ActionKind::Cargo {
                offline_first: true,
            },
            phase: WorkflowPhase::Host,
            program: program.display().to_string(),
            args: vec!["test".into(), "--".into(), "--nocapture".into()],
            reason: "executor test".into(),
            failure_hint: "inspect run".into(),
            inputs: vec!["fixture".into()],
            builtin: None,
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
            started_ms: now_ms(),
            started: Instant::now(),
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
    fn cargo_dependency_policy_is_explicit() {
        let mut operation = test_operation(Path::new("cargo"));
        operation.args = vec!["fmt".into(), "--check".into()];
        operation.action = ActionKind::Cargo {
            offline_first: false,
        };
        assert!(!is_cargo_dependency_operation(&operation));
        assert_eq!(operation.args, ["fmt", "--check"]);
    }

    #[test]
    fn command_heartbeat_names_the_current_operation() {
        let operation = test_operation(Path::new("cargo"));
        assert_eq!(operation_heartbeat(&operation), "Test fake crate");
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
    fn only_deterministic_failures_are_cacheable() {
        assert!(deterministic_failure(
            "error: test_failure (exit 101): assertion failed"
        ));
        assert!(deterministic_failure(
            "error: command_failed (exit 1): compiler error"
        ));
        assert!(!deterministic_failure(
            "error: command_failed: operation timed out"
        ));
        assert!(!deterministic_failure(
            "error: command_failed: permission denied"
        ));
        assert!(!deterministic_failure("error: network_required"));
    }

    #[test]
    fn parallel_scheduler_overlaps_work_and_preserves_result_order() {
        let active = Arc::new(AtomicU64::new(0));
        let maximum = Arc::new(AtomicU64::new(0));
        let items = [0_u64, 1, 2, 3];
        let results = run_parallel_ordered(&items, 4, |item| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            if *item == 1 || *item == 3 {
                Err(format!("failure-{item}"))
            } else {
                Ok(())
            }
        });
        assert!(maximum.load(Ordering::SeqCst) > 1);
        assert_eq!(results[1].as_ref().unwrap_err(), "failure-1");
        assert_eq!(results[3].as_ref().unwrap_err(), "failure-3");
    }

    #[test]
    fn fingerprint_context_hashes_overlapping_inputs_once_at_scale() {
        let (root, cargo) = fake_cargo("#!/bin/sh\nexit 0\n");
        let fixture = root.join("fixture");
        fs::create_dir_all(&fixture).unwrap();
        let mut changes = Vec::new();
        for index in 0..1_000 {
            let relative = PathBuf::from(format!("fixture/file-{index}.txt"));
            fs::write(root.join(&relative), format!("value-{index}")).unwrap();
            changes.push(relative);
        }
        let operations = vec![test_operation(&cargo), test_operation(&cargo)];
        let context = FingerprintContext::new(&root, &operations, &changes).unwrap();
        assert_eq!(context.files.len(), changes.len());
        assert!(context
            .files
            .values()
            .all(|fingerprint| fingerprint.starts_with("file:")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execution_rejects_actions_above_intent_risk() {
        let (root, cargo) = fake_cargo("#!/bin/sh\nexit 0\n");
        let evidence = Evidence::open_at(&root.join("evidence")).unwrap();
        let request = RawRequest {
            id: "risk-run".into(),
            args: vec!["agent-cli".into(), "check".into()],
            started_ms: now_ms(),
            started: Instant::now(),
        };
        evidence.begin_request(&request).unwrap();
        let mut operation = test_operation(&cargo);
        operation.risk = Risk::DeviceWrite;
        let plan = Plan {
            intent: Intent::Check {
                scope: Scope::Paths(vec!["fixture".into()]),
            },
            operations: vec![operation],
            external_requirements: Vec::new(),
        };
        let mut reporter = Reporter::new(&evidence, OutputFormat::Human, &request.id);
        let error = execute(&evidence, &request.id, &root, &plan, &mut reporter).unwrap_err();
        assert!(error.contains("policy_rejected"));
        assert!(evidence
            .run_detail(&request.id)
            .unwrap()
            .unwrap()
            .commands
            .is_empty());
        fs::remove_dir_all(root).unwrap();
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
