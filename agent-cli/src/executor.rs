// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{Evidence, now_ms};
use crate::model::{ActionKind, Operation, Outcome, Plan, WorkflowPhase};
use crate::progress::{EventKind, Reporter};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const VALIDATION_CACHE_SCHEMA: &str = "planner-schema-4";
const FAST_CARGO_POLICY_SCHEMA: &str = "fast-assurance-v1";
const FAST_CARGO_ENVIRONMENT: &[(&str, &str)] = &[
    ("CARGO_INCREMENTAL", "1"),
    ("CARGO_PROFILE_DEV_OPT_LEVEL", "0"),
    ("CARGO_PROFILE_DEV_DEBUG", "0"),
    ("CARGO_PROFILE_DEV_LTO", "false"),
    ("CARGO_PROFILE_DEV_INCREMENTAL", "true"),
    ("CARGO_PROFILE_DEV_CODEGEN_UNITS", "256"),
    ("CARGO_PROFILE_TEST_OPT_LEVEL", "0"),
    ("CARGO_PROFILE_TEST_DEBUG", "0"),
    ("CARGO_PROFILE_TEST_LTO", "false"),
    ("CARGO_PROFILE_TEST_INCREMENTAL", "true"),
    ("CARGO_PROFILE_TEST_CODEGEN_UNITS", "256"),
];
const GIT_LOCAL_ENVIRONMENT: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

pub fn execute(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    plan: &Plan,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    execute_with_changes(evidence, request_id, repository, plan, &[], reporter)
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
    let command = match &plan.request {
        crate::model::AssuranceRequest::PrePush { .. } => "pre-push",
        crate::model::AssuranceRequest::CiHostAssurance { .. } => "ci host-assurance",
        _ => "check",
    };
    let collect_all = collect_all_failures(&plan.request);
    let fingerprints = FingerprintContext::new(repository, &plan.operations, changes)?;
    let mut failures = Vec::new();
    let mut phase = None;
    let mut index = 0;
    while index < plan.operations.len() {
        let operation = &plan.operations[index];
        let operation_phase = operation.workflow_phase();
        if phase != Some(operation_phase) {
            phase = Some(operation_phase);
            reporter.emit(
                EventKind::Progress,
                command,
                workflow_phase_label(operation_phase),
                None,
            )?;
        }
        if operation.builtin.is_some() && operation.risk == crate::model::Risk::ReadOnly {
            let start = index;
            while index < plan.operations.len()
                && plan.operations[index].workflow_phase() == operation.workflow_phase()
                && plan.operations[index].builtin.is_some()
                && plan.operations[index].risk == crate::model::Risk::ReadOnly
            {
                index += 1;
            }
            let batch_failures = run_builtin_batch(
                evidence,
                request_id,
                repository,
                &plan.operations[start..index],
                &fingerprints,
                reporter,
                command,
                collect_all,
            )?;
            if !batch_failures.is_empty() {
                if !collect_all {
                    return Err(render_single_failure(
                        evidence,
                        request_id,
                        command,
                        &batch_failures[0],
                    )?);
                }
                failures.extend(batch_failures);
            }
            continue;
        }
        let heartbeat = operation_heartbeat(operation);
        let cache = operation_cache_key(operation, &fingerprints)?;
        if let Some(fingerprint) = cache.as_ref()
            && evidence.has_cached_validation_success(&operation.id, fingerprint)?
        {
            evidence.record_reused_command(
                request_id,
                &operation.id,
                &operation.program,
                &operation.args,
                operation.resource_class().as_str(),
            )?;
            index += 1;
            continue;
        }
        if let Some(fingerprint) = cache.as_ref()
            && claim_or_wait(
                evidence,
                request_id,
                reporter,
                command,
                operation,
                fingerprint,
            )?
        {
            index += 1;
            continue;
        }
        if let Err(error) = run_operation(
            evidence,
            request_id,
            repository,
            operation,
            command,
            heartbeat,
            reporter,
            cache.as_deref(),
        ) {
            if let Some(fingerprint) = cache.as_ref() {
                evidence.release_validation(&operation.id, fingerprint, request_id)?;
            }
            match error {
                OperationRunError::Failure(detail) => {
                    let failure = CheckFailure {
                        title: operation.title.clone(),
                        detail,
                    };
                    if !collect_all {
                        return Err(render_single_failure(
                            evidence, request_id, command, &failure,
                        )?);
                    }
                    failures.push(failure);
                    index += 1;
                    continue;
                }
                OperationRunError::Infrastructure(error) => return Err(error),
            }
        }
        if operation.risk == crate::model::Risk::ReadOnly
            && let Some(fingerprint) = cache
        {
            evidence.cache_validation_success(&operation.id, &fingerprint)?;
            evidence.release_validation(&operation.id, &fingerprint, request_id)?;
        }
        index += 1;
    }
    if !failures.is_empty() {
        return Err(render_failure_report(
            command,
            plan.operations.len(),
            &failures,
            request_id,
        ));
    }
    reporter.emit(EventKind::Completed, command, "passed", Some(100))?;
    Ok(Outcome::Passed)
}

#[derive(Debug)]
struct CheckFailure {
    title: String,
    detail: String,
}

#[derive(Debug)]
enum OperationRunError {
    Failure(String),
    Infrastructure(String),
}

const fn collect_all_failures(request: &crate::model::AssuranceRequest) -> bool {
    matches!(
        request,
        crate::model::AssuranceRequest::PrePush { .. }
            | crate::model::AssuranceRequest::CiHostAssurance { .. }
    )
}

impl From<String> for OperationRunError {
    fn from(error: String) -> Self {
        Self::Infrastructure(error)
    }
}

fn render_failure_report(
    command: &str,
    total: usize,
    failures: &[CheckFailure],
    request_id: &str,
) -> String {
    use std::fmt::Write as _;

    let mut report = format!(
        "{command}: {} of {total} selected checks failed",
        failures.len()
    );
    for (index, failure) in failures.iter().enumerate() {
        let _ = write!(report, "\n\n{}. {}", index + 1, failure.title);
        for line in failure.detail.lines() {
            let _ = write!(report, "\n   {line}");
        }
    }
    let _ = write!(report, "\n\nnext: scripts/agent run show {request_id}");
    report
}

fn render_single_failure(
    evidence: &Evidence,
    request_id: &str,
    command: &str,
    failure: &CheckFailure,
) -> Result<String, String> {
    let next = if failure.detail.contains("error: network_required") {
        format!(
            "rerun with network access: {}",
            crate::shell::agent_retry_command(&evidence.request_args(request_id)?)
        )
    } else {
        format!("scripts/agent run show {request_id}")
    };
    Ok(format!(
        "{command}: failed — {}\n{}\nnext: {next}",
        failure.title, failure.detail
    ))
}

const fn workflow_phase_label(phase: WorkflowPhase) -> &'static str {
    match phase {
        WorkflowPhase::Cheap => "cheap checks",
        WorkflowPhase::Host => "host validation",
        WorkflowPhase::Expensive => "building",
        WorkflowPhase::External => "waiting for external validation",
        WorkflowPhase::Device => "device operation",
    }
}

fn claim_or_wait(
    evidence: &Evidence,
    request_id: &str,
    reporter: &mut Reporter<'_>,
    phase: &str,
    operation: &Operation,
    fingerprint: &str,
) -> Result<bool, String> {
    loop {
        if evidence.claim_validation(&operation.id, fingerprint, request_id)? {
            return Ok(false);
        }
        let queued = Instant::now();
        let owner = evidence.validation_owner(&operation.id, fingerprint)?;
        let result = wait_for_validation(evidence, reporter, phase, operation, fingerprint)?;
        evidence.add_queue_ms(
            request_id,
            i64::try_from(queued.elapsed().as_millis()).unwrap_or(i64::MAX),
        )?;
        if let Some(owner) = owner.as_deref() {
            evidence.record_joined_command(
                request_id,
                owner,
                &operation.id,
                &operation.program,
                &operation.args,
                operation.resource_class().as_str(),
            )?;
        }
        if result {
            return Ok(true);
        }
    }
}

fn wait_for_validation(
    evidence: &Evidence,
    reporter: &mut Reporter<'_>,
    phase: &str,
    operation: &Operation,
    fingerprint: &str,
) -> Result<bool, String> {
    let started = Instant::now();
    let mut next_progress = Duration::from_secs(10);
    while started.elapsed() < Duration::from_secs(31 * 60) {
        if evidence.has_cached_validation_success(&operation.id, fingerprint)? {
            return Ok(true);
        }
        if evidence
            .validation_owner(&operation.id, fingerprint)?
            .is_none()
        {
            return Ok(false);
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

fn run_builtin_batch(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    operations: &[Operation],
    fingerprints: &FingerprintContext,
    reporter: &mut Reporter<'_>,
    command: &str,
    collect_all: bool,
) -> Result<Vec<CheckFailure>, String> {
    let mut pending = Vec::new();
    let mut failures = Vec::new();
    for operation in operations {
        let cache = operation_cache_key(operation, fingerprints)?;
        if let Some(fingerprint) = cache.as_ref() {
            if evidence.has_cached_validation_success(&operation.id, fingerprint)? {
                evidence.record_reused_command(
                    request_id,
                    &operation.id,
                    &operation.program,
                    &operation.args,
                    operation.resource_class().as_str(),
                )?;
                continue;
            }
            if claim_or_wait(
                evidence,
                request_id,
                reporter,
                command,
                operation,
                fingerprint,
            )? {
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
        for ((operation, _, cache), result) in chunk.iter().zip(results) {
            if let Err(error) = result {
                if let Some(fingerprint) = cache {
                    evidence.release_validation(&operation.id, fingerprint, request_id)?;
                }
                let log_path = evidence.log_path(request_id, &operation.id);
                std::fs::write(&log_path, &error).map_err(|write_error| {
                    format!(
                        "cannot write builtin failure log {}: {write_error}",
                        log_path.display()
                    )
                })?;
                failures.push(CheckFailure {
                    title: operation.title.clone(),
                    detail: format!(
                        "error: check_failure\nsummary: {error}\nlog: {}",
                        log_path.display()
                    ),
                });
            } else if let Some(fingerprint) = cache {
                evidence.cache_validation_success(&operation.id, fingerprint)?;
                evidence.release_validation(&operation.id, fingerprint, request_id)?;
            }
        }
        if !collect_all && !failures.is_empty() {
            for (operation, _, cache) in &pending {
                if let Some(fingerprint) = cache {
                    evidence.release_validation(&operation.id, fingerprint, request_id)?;
                }
            }
            break;
        }
    }
    Ok(failures)
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
    operation: &Operation,
    fingerprints: &FingerprintContext,
) -> Result<Option<String>, String> {
    operation_cache_key_with_schema(operation, fingerprints, VALIDATION_CACHE_SCHEMA)
}

fn operation_cache_key_with_schema(
    operation: &Operation,
    fingerprints: &FingerprintContext,
    schema: &str,
) -> Result<Option<String>, String> {
    if !is_cacheable(operation) || operation.inputs.is_empty() {
        return Ok(None);
    }
    let files = fingerprints
        .files
        .iter()
        .filter(|(path, _)| operation.inputs.iter().any(|input| path.starts_with(input)))
        .collect::<Vec<_>>();
    let canonical = serde_json::to_vec(&(
        schema,
        FAST_CARGO_POLICY_SCHEMA,
        &operation.id,
        &operation.program,
        &operation.args,
        &operation.inputs,
        &fingerprints.toolchain,
        &fingerprints.environment,
        files,
    ))
    .map_err(|error| format!("cache_identity_failed: {error}"))?;
    let digest = Sha256::digest(canonical);
    Ok(Some(
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    ))
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
        let roots: std::collections::BTreeSet<_> = operations
            .iter()
            .filter(|operation| is_cacheable(operation))
            .flat_map(|operation| operation.inputs.iter().map(PathBuf::from))
            .collect();
        let mut tracked = Command::new("git");
        tracked
            .current_dir(repository)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
            ])
            .args(&roots);
        let output = tracked
            .output()
            .map_err(|error| format!("cannot enumerate validation inputs: {error}"))?;
        let mut files = BTreeMap::new();
        let paths: Vec<PathBuf> = if output.status.success() {
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned()))
                .collect()
        } else {
            changes.to_vec()
        };
        for path in paths {
            files.insert(path.clone(), fingerprint_path(&repository.join(path))?);
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
    reporter: &mut Reporter<'_>,
    fingerprint: Option<&str>,
) -> Result<(), OperationRunError> {
    if let Some(builtin) = operation.builtin {
        return match crate::checks::execute(builtin, repository, reporter) {
            Ok(()) => Ok(()),
            Err(error) => {
                let log_path = evidence.log_path(request_id, &operation.id);
                std::fs::write(&log_path, &error).map_err(|write_error| {
                    OperationRunError::Infrastructure(format!(
                        "cannot write builtin failure log {}: {write_error}",
                        log_path.display()
                    ))
                })?;
                Err(OperationRunError::Failure(format!(
                    "error: check_failure\nsummary: {error}\nlog: {}",
                    log_path.display()
                )))
            }
        };
    }
    let log_path = evidence.log_path(request_id, &operation.id);
    File::create(&log_path)
        .map_err(|error| OperationRunError::Infrastructure(error.to_string()))?;
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
        fingerprint,
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
            fingerprint,
        )?;
        if online_status.success() {
            return Ok(());
        }
        let online_output = read_log_from(&log_path, online_start)?;
        return Err(OperationRunError::Failure(failure_message(
            operation,
            &log_path,
            online_status.code().unwrap_or(1),
            &online_output,
        )?));
    }
    Err(OperationRunError::Failure(failure_message(
        operation,
        &log_path,
        first_status.code().unwrap_or(1),
        &first_output,
    )?))
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
    fingerprint: Option<&str>,
) -> Result<std::process::ExitStatus, OperationRunError> {
    let mut log = OpenOptions::new()
        .append(true)
        .open(log_path)
        .map_err(|error| OperationRunError::Infrastructure(error.to_string()))?;
    writeln!(log, "=== agent-cli attempt: {attempt} ===")
        .map_err(|error| OperationRunError::Infrastructure(error.to_string()))?;
    let started = now_ms();
    let command_id = evidence.begin_command(
        request_id,
        &operation.id,
        &operation.program,
        args,
        Some(log_path),
        operation.resource_class().as_str(),
    )?;
    let mut command = Command::new(&operation.program);
    command
        .args(args)
        .current_dir(repository)
        .env("MISTER_AGENT_PARENT_REQUEST_ID", request_id);
    apply_operation_environment(&mut command, operation);
    scrub_git_local_environment(&mut command);
    if attempt == "network-fallback" {
        command.env("CARGO_NET_RETRY", "0");
    }
    let child = command
        .stdout(Stdio::from(log.try_clone().map_err(|error| {
            OperationRunError::Infrastructure(error.to_string())
        })?))
        .stderr(Stdio::from(log))
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            evidence.finish_command(command_id, started, 127)?;
            return Err(OperationRunError::Failure(format!(
                "error: command_launch_failed\nsummary: {error}\nlog: {}",
                log_path.display()
            )));
        }
    };
    let status = crate::process::wait(
        &mut child,
        None,
        &operation.program,
        Some(std::time::Duration::from_millis(
            crate::progress::HEARTBEAT_MS,
        )),
        || {
            if let Some(fingerprint) = fingerprint {
                evidence.heartbeat_validation(&operation.id, fingerprint, request_id)?;
            }
            reporter
                .emit(EventKind::Progress, phase, heartbeat, None)
                .map_err(|error| error.to_string())
        },
    )?;
    let code = status.code().unwrap_or(1);
    evidence.finish_command(command_id, started, code)?;
    Ok(status)
}

fn apply_operation_environment(command: &mut Command, operation: &Operation) {
    if matches!(operation.action, ActionKind::Cargo { .. }) {
        command.envs(FAST_CARGO_ENVIRONMENT.iter().copied());
    }
}

fn scrub_git_local_environment(command: &mut Command) {
    for variable in GIT_LOCAL_ENVIRONMENT {
        command.env_remove(variable);
    }
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
        || lower.contains("failed to select a version") && lower.contains("offline mode")
        || lower.contains("failed to download") && lower.contains("offline")
        || lower.contains("can't checkout from") && lower.contains("offline mode")
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
    operation: &Operation,
    log_path: &Path,
    code: i32,
    output: &str,
) -> Result<String, String> {
    let classification = failure_classification(operation, code, output);
    Ok(format!(
        "error: {classification} (exit {code})\nsummary: {}\nlog: {}",
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
    use crate::model::{ActionKind, AssuranceRequest, Risk, Scope, WorkflowPhase};
    use crate::request::RawRequest;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
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

    #[test]
    fn validation_children_do_not_inherit_hook_repository_identity() {
        let mut command = Command::new("fixture");
        for variable in GIT_LOCAL_ENVIRONMENT {
            command.env(variable, "inherited-from-hook");
        }
        scrub_git_local_environment(&mut command);
        let removed = command
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect::<Vec<_>>();
        assert_eq!(removed.len(), GIT_LOCAL_ENVIRONMENT.len());
        assert!(removed.contains(&std::ffi::OsStr::new("GIT_DIR")));
        assert!(removed.contains(&std::ffi::OsStr::new("GIT_WORK_TREE")));
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
            args: vec![
                "agent-cli".into(),
                "ci".into(),
                "host-assurance".into(),
                "--paths".into(),
                "fixture.rs".into(),
            ],
            started_ms: now_ms(),
            started: Instant::now(),
        };
        evidence.begin_request(&request).unwrap();
        evidence
            .record_intent(
                &request.id,
                &AssuranceRequest::CiHostAssurance {
                    scope: Scope::Paths(vec!["fixture.rs".into()]),
                },
            )
            .unwrap();
        let plan = Plan {
            request: AssuranceRequest::CiHostAssurance {
                scope: Scope::Paths(vec!["fixture.rs".into()]),
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
    fn cargo_children_receive_the_exact_fast_assurance_profile() {
        let mut command = Command::new("fixture");
        for (name, _) in FAST_CARGO_ENVIRONMENT {
            command.env(name, "external-slow-value");
        }
        let operation = test_operation(Path::new("cargo"));
        apply_operation_environment(&mut command, &operation);
        let environment = command
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        for (name, expected) in FAST_CARGO_ENVIRONMENT {
            assert_eq!(environment.get(*name).map(String::as_str), Some(*expected));
        }
        assert!(!environment.contains_key("CARGO_PROFILE_DEV_DEBUG_ASSERTIONS"));
        assert!(!environment.contains_key("CARGO_PROFILE_DEV_OVERFLOW_CHECKS"));
    }

    #[test]
    fn non_cargo_children_do_not_receive_assurance_profile_overrides() {
        let mut command = Command::new("fixture");
        let mut operation = test_operation(Path::new("fixture"));
        operation.action = ActionKind::Script;
        apply_operation_environment(&mut command, &operation);
        let configured = command
            .get_envs()
            .filter_map(|(name, value)| value.map(|_| name))
            .collect::<Vec<_>>();
        assert!(configured.is_empty());
    }

    #[test]
    fn child_commands_receive_parent_request_identity() {
        let (result, _, _) = execute_fake_cargo(
            "#!/bin/sh\n[ \"$MISTER_AGENT_PARENT_REQUEST_ID\" = \"test-run\" ]\n",
        );
        assert_eq!(result.unwrap(), Outcome::Passed);
    }

    #[test]
    fn non_assurance_request_remains_fail_fast() {
        let (root, failing) = fake_cargo("#!/bin/sh\nexit 1\n");
        let marker = root.join("host-ran");
        let host = root.join("host");
        fs::write(&host, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        let mut permissions = fs::metadata(&host).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&host, permissions).unwrap();
        let evidence = Evidence::open_at(&root.join("state")).unwrap();
        let request = RawRequest {
            id: "preflight-run".into(),
            args: vec!["agent-cli".into(), "check".into()],
            started_ms: now_ms(),
            started: Instant::now(),
        };
        evidence.begin_request(&request).unwrap();
        let mut cheap = test_operation(&failing);
        cheap.phase = WorkflowPhase::Cheap;
        let mut host_operation = test_operation(&host);
        host_operation.id = "host.operation".into();
        host_operation.action = ActionKind::Script;
        let plan = Plan {
            request: AssuranceRequest::Plan {
                scope: Scope::WorkingTree,
            },
            operations: vec![cheap, host_operation],
            external_requirements: Vec::new(),
        };
        let mut reporter = Reporter::new(&evidence, OutputFormat::Human, &request.id);
        assert!(execute(&evidence, &request.id, &root, &plan, &mut reporter).is_err());
        assert!(!marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_push_and_ci_collect_failures_but_plan_does_not() {
        assert!(collect_all_failures(&AssuranceRequest::PrePush {
            remote: "origin".into(),
        }));
        assert!(collect_all_failures(&AssuranceRequest::CiHostAssurance {
            scope: Scope::WorkingTree,
        }));
        assert!(!collect_all_failures(&AssuranceRequest::Plan {
            scope: Scope::WorkingTree,
        }));
    }

    #[test]
    fn ci_runs_later_phases_and_reports_all_failures_in_plan_order() {
        let (root, cheap_program) = fake_cargo("#!/bin/sh\necho 'cheap lint failed' >&2\nexit 2\n");
        let host_marker = root.join("host-ran");
        let host_program = root.join("host-check");
        fs::write(
            &host_program,
            format!("#!/bin/sh\ntouch '{}'\n", host_marker.display()),
        )
        .unwrap();
        let expensive_marker = root.join("expensive-ran");
        let expensive_program = root.join("expensive-check");
        fs::write(
            &expensive_program,
            format!(
                "#!/bin/sh\ntouch '{}'\necho 'test result: failed' >&2\nexit 7\n",
                expensive_marker.display()
            ),
        )
        .unwrap();
        for program in [&host_program, &expensive_program] {
            let mut permissions = fs::metadata(program).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(program, permissions).unwrap();
        }

        let evidence = Evidence::open_at(&root.join("state")).unwrap();
        let request = RawRequest {
            id: "collect-all-run".into(),
            args: vec!["agent-cli".into(), "ci".into(), "host-assurance".into()],
            started_ms: now_ms(),
            started: Instant::now(),
        };
        evidence.begin_request(&request).unwrap();

        let mut cheap = test_operation(&cheap_program);
        cheap.id = "cheap.lint".into();
        cheap.title = "Cheap lint".into();
        cheap.phase = WorkflowPhase::Cheap;
        cheap.action = ActionKind::Script;
        cheap.args.clear();

        let mut host = test_operation(&host_program);
        host.id = "host.success".into();
        host.title = "Host success".into();
        host.action = ActionKind::Script;
        host.args.clear();

        let mut missing = test_operation(&root.join("missing-check"));
        missing.id = "expensive.missing".into();
        missing.title = "Missing tool".into();
        missing.phase = WorkflowPhase::Expensive;
        missing.action = ActionKind::Script;
        missing.args.clear();

        let mut expensive = test_operation(&expensive_program);
        expensive.id = "expensive.test".into();
        expensive.title = "Late test".into();
        expensive.phase = WorkflowPhase::Expensive;
        expensive.action = ActionKind::Script;
        expensive.args = vec!["test".into()];

        let plan = Plan {
            request: AssuranceRequest::CiHostAssurance {
                scope: Scope::WorkingTree,
            },
            operations: vec![cheap.clone(), host.clone(), missing, expensive],
            external_requirements: Vec::new(),
        };
        let mut reporter = Reporter::new(&evidence, OutputFormat::Human, &request.id);
        let error = execute(&evidence, &request.id, &root, &plan, &mut reporter).unwrap_err();

        assert!(host_marker.exists());
        assert!(expensive_marker.exists());
        assert!(error.contains("ci host-assurance: 3 of 4 selected checks failed"));
        let cheap_position = error.find("1. Cheap lint").unwrap();
        let missing_position = error.find("2. Missing tool").unwrap();
        let expensive_position = error.find("3. Late test").unwrap();
        assert!(cheap_position < missing_position && missing_position < expensive_position);
        assert!(error.contains("error: command_launch_failed"));
        assert!(error.contains("error: test_failure (exit 7)"));
        assert_eq!(
            error
                .matches("next: scripts/agent run show collect-all-run")
                .count(),
            1
        );

        let fingerprints = FingerprintContext::new(&root, &plan.operations, &[]).unwrap();
        let host_cache = operation_cache_key(&host, &fingerprints).unwrap().unwrap();
        let cheap_cache = operation_cache_key(&cheap, &fingerprints).unwrap().unwrap();
        assert!(
            evidence
                .has_cached_validation_success(&host.id, &host_cache)
                .unwrap()
        );
        assert!(
            !evidence
                .has_cached_validation_success(&cheap.id, &cheap_cache)
                .unwrap()
        );
        assert!(
            evidence
                .validation_owner(&cheap.id, &cheap_cache)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn combined_report_orders_builtin_and_command_failures_with_one_next_action() {
        let failures = vec![
            CheckFailure {
                title: "Builtin lint".into(),
                detail: "error: check_failure\nsummary: policy mismatch\nlog: builtin.log".into(),
            },
            CheckFailure {
                title: "Command test".into(),
                detail: "error: test_failure (exit 1)\nsummary: failed assertion\nlog: test.log"
                    .into(),
            },
        ];
        let report = render_failure_report("pre-push", 5, &failures, "report-run");
        assert!(report.contains("pre-push: 2 of 5 selected checks failed"));
        assert!(report.find("1. Builtin lint").unwrap() < report.find("2. Command test").unwrap());
        assert_eq!(
            report
                .matches("next: scripts/agent run show report-run")
                .count(),
            1
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
        assert!(
            detail.commands[0]
                .args
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "--offline")
        );
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
    fn offline_version_selection_miss_retries_online() {
        let script = "#!/bin/sh\ncase \" $* \" in\n  *\" --offline \"*) echo 'error: failed to select a version for cc; candidate versions found which did not match; note: offline mode can sometimes cause surprising resolution failures' >&2; exit 101;;\n  *) exit 0;;\nesac\n";
        let (result, detail, log) = execute_fake_cargo(script);
        assert_eq!(result.unwrap(), Outcome::Passed);
        assert_eq!(detail.commands.len(), 2);
        assert!(log.contains("=== agent-cli attempt: network-fallback ==="));
    }

    #[test]
    fn offline_git_checkout_miss_retries_online() {
        let script = "#!/bin/sh\ncase \" $* \" in\n  *\" --offline \"*) echo \"failed to load source for dependency 'pprof'; can't checkout from 'https://example.invalid/pprof': you are in the offline mode (--offline)\" >&2; exit 101;;\n  *) exit 0;;\nesac\n";
        let (result, detail, log) = execute_fake_cargo(script);
        assert_eq!(result.unwrap(), Outcome::Passed);
        assert_eq!(detail.commands.len(), 2);
        assert!(log.contains("=== agent-cli attempt: network-fallback ==="));
    }

    #[test]
    fn network_failure_after_cache_miss_is_not_a_test_failure() {
        let script = "#!/bin/sh\ncase \" $* \" in\n  *\" --offline \"*) echo 'attempting to make an HTTP request, but --offline was specified' >&2;;\n  *) echo \"unable to update registry crates-io: Couldn't resolve host: index.crates.io\" >&2;;\nesac\nexit 101\n";
        let (result, detail, _) = execute_fake_cargo(script);
        let error = result.unwrap_err();
        assert!(error.contains("error: network_required"));
        assert!(error.contains("next: scripts/agent run show test-run"));
        assert!(!error.contains("rerun with network access"));
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
        assert!(
            context
                .files
                .values()
                .all(|fingerprint| fingerprint.starts_with("file:"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_fingerprint_includes_unchanged_git_scope_dependencies() {
        let (root, cargo) = fake_cargo("#!/bin/sh\nexit 0\n");
        fs::create_dir_all(root.join("fixture")).unwrap();
        fs::write(root.join("fixture/a.rs"), "a").unwrap();
        fs::write(root.join("fixture/b.rs"), "b").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Agent"],
            vec!["config", "user.email", "agent@example.invalid"],
            vec!["add", "."],
            vec!["commit", "-qm", "fixture"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let operation = test_operation(&cargo);
        let changes = vec![PathBuf::from("fixture/a.rs")];
        let first =
            FingerprintContext::new(&root, std::slice::from_ref(&operation), &changes).unwrap();
        let first_key = operation_cache_key(&operation, &first).unwrap();
        fs::write(root.join("fixture/b.rs"), "changed").unwrap();
        let second =
            FingerprintContext::new(&root, std::slice::from_ref(&operation), &changes).unwrap();
        let second_key = operation_cache_key(&operation, &second).unwrap();
        assert_ne!(first_key, second_key);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_path_cache_rekeys_when_launcher_rust_changes() {
        let (root, cargo) = fake_cargo("#!/bin/sh\nexit 0\n");
        fs::create_dir_all(root.join("apps/mister/src/ui_runner")).unwrap();
        fs::create_dir_all(root.join("apps/mister/ui")).unwrap();
        fs::write(
            root.join("apps/mister/src/ui_runner/launcher_loop.rs"),
            "let compatibility_active = true;\n",
        )
        .unwrap();
        fs::write(
            root.join("apps/mister/ui/launcher.slint"),
            "export component Launcher {}\n",
        )
        .unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Agent"],
            vec!["config", "user.email", "agent@example.invalid"],
            vec!["add", "."],
            vec!["commit", "-qm", "fixture"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let mut operation = test_operation(&cargo);
        operation.inputs = vec!["apps/mister".into()];
        let changes = vec![PathBuf::from("apps/mister/ui/launcher.slint")];
        let first =
            FingerprintContext::new(&root, std::slice::from_ref(&operation), &changes).unwrap();
        let first_key = operation_cache_key(&operation, &first)
            .unwrap()
            .expect("explicit path checks are cacheable");
        assert_eq!(first_key.len(), 64);
        assert!(first_key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let old_schema_key =
            operation_cache_key_with_schema(&operation, &first, "planner-schema-2")
                .unwrap()
                .expect("old explicit path key");
        assert_ne!(first_key, old_schema_key);
        fs::write(
            root.join("apps/mister/src/ui_runner/launcher_loop.rs"),
            "let compatibility_prompt_visible = true;\n",
        )
        .unwrap();
        let second =
            FingerprintContext::new(&root, std::slice::from_ref(&operation), &changes).unwrap();
        let second_key = operation_cache_key(&operation, &second)
            .unwrap()
            .expect("explicit path checks are cacheable");
        assert_ne!(first_key, second_key);
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
